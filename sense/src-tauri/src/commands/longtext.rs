//! 长文翻译：整篇一次送，边生成边推给前端。
//!
//! 不切块。切块会让第二段丢失第一段的指代和术语，译文与原文意思偏；
//! 代价是输出更长，所以上限放到 8192，并全程流式。

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::providers as provider_registry;
use crate::providers::openai_compat;
use crate::secrets;

/// 长文的输出上限。一篇 3000 词的文章译文约 8000 token。
const LONG_MAX_TOKENS: u32 = 8192;

/// 每次长文翻译领一个编号；「停止」把当前及更早的编号都标成取消。
///
/// 以前是一个全局布尔：下一次翻译一开始就把它复位，上一条流可能再也停不下来，
/// 而长文的时限现在有十几分钟，孤儿流会一直计费。
static LATEST: AtomicU64 = AtomicU64::new(0);
static CANCELLED_UPTO: AtomicU64 = AtomicU64::new(0);

fn begin() -> u64 {
    LATEST.fetch_add(1, Ordering::SeqCst) + 1
}

fn is_cancelled(id: u64) -> bool {
    CANCELLED_UPTO.load(Ordering::SeqCst) >= id
}

/// 用户点「停止」，或离开阅读视图、开始新的查询。
#[tauri::command]
pub fn cancel_translate() {
    CANCELLED_UPTO.store(LATEST.load(Ordering::SeqCst), Ordering::SeqCst);
}

/// 长文结果：除了译文，还要告诉前端它是不是完整的。
#[derive(Clone, Serialize)]
pub struct LongTextResult {
    pub text: String,
    /// 撞到输出上限（finish_reason = length）。
    pub truncated: bool,
    /// 其它没有正常写完的原因：被停止、被内容策略中止、没收到结束标记……正常结束为 None。
    pub incomplete: Option<String>,
}

/// 结束方式 → 「译文不完整」的原因。正常结束（stop，或只收到 [DONE]）返回 None；
/// length 由 `truncated` 单独提示，这里不重复。
fn incomplete_reason(finish_reason: Option<&str>, done: bool, cancelled: bool) -> Option<String> {
    if cancelled {
        return Some("你点了停止".to_string());
    }
    match finish_reason {
        Some("stop") | Some("length") => None,
        Some("content_filter") => Some("模型服务以「内容策略」中止了输出".to_string()),
        Some(other) => Some(format!("模型服务以「{other}」结束了输出")),
        None if done => None,
        None => Some("没有收到结束标记，连接可能提前关闭了".to_string()),
    }
}

#[derive(Clone, Serialize)]
struct LongTextDelta {
    request_id: String,
    text: String,
}

fn looks_chinese(text: &str) -> bool {
    let cjk = text.chars().filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch)).count();
    let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    cjk * 2 >= latin && cjk > 0
}

/// 整篇翻译。增量通过 `sense://longtext-delta` 推给右栏。
#[tauri::command]
pub async fn translate_text(
    app: AppHandle,
    text: String,
    request_id: String,
) -> Result<LongTextResult, String> {
    let source = text.trim().to_string();
    if source.is_empty() {
        return Err("还没有内容".to_string());
    }

    // 编号要在读钥匙串之前领：用户在等钥匙串时点的「停止」，也得算到这一条头上
    let id = begin();
    let stopped = move || is_cancelled(id);

    let provider = provider_registry::active(&app)?;
    let key = if provider.requires_key {
        secrets::get_key(&provider.id)?
    } else {
        None
    };

    let target = if looks_chinese(&source) { "英文" } else { "中文" };
    let system = format!(
        "你是翻译引擎。把 user 给的整篇文本翻译成{target}。\n\
         要求：\n\
         1. 先通读全文再动笔，术语、人名、指代前后保持一致；\n\
         2. 保留原有分段与换行；\n\
         3. 代码、命令、路径、URL、版本号、数字原样保留；\n\
         4. 只输出译文，不要解释、不要加引号或 Markdown 代码块。"
    );

    let reasoning = provider_registry::reasoning_fields(&provider.id);
    let body = openai_compat::build_body(
        &provider.model,
        &system,
        &source,
        0.3,
        reasoning.as_ref(),
        LONG_MAX_TOKENS,
    );

    let emitter = app.clone();
    let rid = request_id.clone();
    let outcome = openai_compat::stream_chat(
        crate::client_for(&provider.base_url),
        &provider,
        key.as_deref(),
        &body,
        &mut move |delta: &str| {
            let _ = emitter.emit(
                "sense://longtext-delta",
                LongTextDelta {
                    request_id: rid.clone(),
                    text: delta.to_string(),
                },
            );
        },
        Some(&stopped),
    )
    .await
    .map_err(|err| err.message())?;

    let finish_reason = outcome.finish_reason.as_deref();
    Ok(LongTextResult {
        truncated: finish_reason == Some("length"),
        incomplete: incomplete_reason(finish_reason, outcome.done, outcome.cancelled),
        text: outcome.text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_abnormal_ending_is_reported() {
        assert_eq!(incomplete_reason(Some("stop"), true, false), None);
        assert_eq!(incomplete_reason(None, true, false), None, "只有 [DONE] 也算正常结束");
        assert_eq!(incomplete_reason(Some("length"), false, false), None, "length 另有 truncated");
        let filtered = incomplete_reason(Some("content_filter"), true, false).expect("要提示");
        assert!(filtered.contains("内容策略"), "{filtered}");
        let other = incomplete_reason(Some("insufficient_system_resource"), true, false).expect("要提示");
        assert!(other.contains("insufficient_system_resource"), "{other}");
        let eof = incomplete_reason(None, false, false).expect("要提示");
        assert!(eof.contains("结束标记"), "{eof}");
        let stopped = incomplete_reason(Some("stop"), true, true).expect("要提示");
        assert!(stopped.contains("停止"), "{stopped}");
    }

    #[test]
    fn stop_only_cancels_requests_already_started() {
        let first = begin();
        cancel_translate();
        let second = begin();
        assert!(is_cancelled(first), "点停止之前开始的那条要停");
        assert!(!is_cancelled(second), "之后开始的新翻译不受影响");
    }

    #[test]
    fn target_language_follows_source() {
        assert!(looks_chinese("这是一段中文"));
        assert!(!looks_chinese("this is english"));
        // 中英混排偏英文时仍译成中文
        assert!(!looks_chinese("Rust 的所有权模型 the ownership model in Rust"));
    }
}
