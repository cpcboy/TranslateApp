//! 长文翻译：整篇一次送，边生成边推给前端。
//!
//! 不切块。切块会让第二段丢失第一段的指代和术语，译文与原文意思偏；
//! 代价是输出更长，所以上限放到 8192，并全程流式。

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::providers as provider_registry;
use crate::providers::openai_compat;
use crate::secrets;

/// 长文的输出上限。一篇 3000 词的文章译文约 8000 token。
const LONG_MAX_TOKENS: u32 = 8192;

/// 当前这次长文翻译是否被用户取消。全局只有一个长文请求，够用。
static CANCELLED: AtomicBool = AtomicBool::new(false);

/// 用户点「停止」。
#[tauri::command]
pub fn cancel_translate() {
    CANCELLED.store(true, Ordering::Relaxed);
}

/// 长文结果：除了译文，还要告诉前端有没有被输出上限截断。
#[derive(Clone, Serialize)]
pub struct LongTextResult {
    pub text: String,
    pub truncated: bool,
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

    CANCELLED.store(false, Ordering::Relaxed);

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
        Some(&CANCELLED),
    )
    .await
    .map_err(|err| err.message())?;

    let truncated = outcome
        .finish_reason
        .as_deref()
        .map(|reason| reason == "length")
        .unwrap_or(false);

    Ok(LongTextResult {
        text: outcome.text,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_language_follows_source() {
        assert!(looks_chinese("这是一段中文"));
        assert!(!looks_chinese("this is english"));
        // 中英混排偏英文时仍译成中文
        assert!(!looks_chinese("Rust 的所有权模型 the ownership model in Rust"));
    }
}
