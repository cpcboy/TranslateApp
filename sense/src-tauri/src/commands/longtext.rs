//! 长文翻译：整篇一次送，边生成边推给前端。
//!
//! 不切块。切块会让第二段丢失第一段的指代和术语，译文与原文意思偏；
//! 代价是输出更长，所以上限放到 8192，并全程流式。

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::language::{load_pref, Languages};
use crate::lang::{self, Lang};
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
    /// 原文语言与目标语言（开始时已经用事件告诉过前端一次，这里再带一份，结果自成一体）。
    #[serde(flatten)]
    pub languages: Languages,
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

/// 翻译开始时告诉前端：原文是什么语言、会译成什么语言（两栏的标题据此标出）。
#[derive(Clone, Serialize)]
struct LongTextLang {
    request_id: String,
    #[serde(flatten)]
    languages: Languages,
}

/// 长文翻译的系统提示词，目标语言按偏好与原文语言取（规则见 `lang.rs`）。
///
/// 头一句是 `tools/mock-provider.mjs` 认「长文翻译」请求的标记（MARKERS.longText），必须原样保留。
/// 目标是中文、英文时与改动前逐字节相同（单测钉着）；模型自己认原文语言，这里不另外告诉它。
fn system_prompt(target: &Lang) -> String {
    format!(
        "你是翻译引擎。把 user 给的整篇文本翻译成{}。\n\
         要求：\n\
         1. 先通读全文再动笔，术语、人名、指代前后保持一致；\n\
         2. 保留原有分段与换行；\n\
         3. 代码、命令、路径、URL、版本号、数字原样保留；\n\
         4. 只输出译文，不要解释、不要加引号或 Markdown 代码块。",
        target.name
    )
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

    // 目标语言 = 偏好 + 原文语言。在读钥匙串、发请求之前就告诉前端：
    // 不管后面成功还是失败，两栏的标题都标得出"日语 → 中文"
    let plan = lang::plan(&source, load_pref(&app));
    let languages = Languages::of(&plan);
    let _ = app.emit(
        "sense://longtext-lang",
        LongTextLang {
            request_id: request_id.clone(),
            languages: languages.clone(),
        },
    );

    let provider = provider_registry::active(&app)?;
    let key = if provider.requires_key {
        secrets::get_key(&provider.id)?
    } else {
        None
    };

    let system = system_prompt(plan.target);

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
    let outcome = openai_compat::stream_chat_adaptive(
        crate::client_for(&provider.base_url),
        &provider,
        key.as_deref(),
        &body,
        reasoning.as_ref(),
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
        languages,
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

    fn target_of(source: &str, pref: &str) -> &'static str {
        lang::plan(source, pref).target.code
    }

    #[test]
    fn default_direction_is_unchanged() {
        // 改动前就是这样判的：中文 → 英文，其它 → 中文；中英混排偏英文时仍译成中文
        assert_eq!(target_of("这是一段中文", lang::AUTO), "en");
        assert_eq!(target_of("this is english", lang::AUTO), "zh");
        assert_eq!(target_of("Rust 的所有权模型 the ownership model in Rust", lang::AUTO), "zh");
    }

    #[test]
    fn target_follows_the_preference_and_falls_back_when_source_is_already_that_language() {
        assert_eq!(target_of("今日は天気がいいですね", lang::AUTO), "zh", "日语默认译成中文（以前会因为带汉字译成英文）");
        assert_eq!(target_of("这是一段中文", "ja"), "ja");
        assert_eq!(target_of("This is an English sentence about the weather.", "fr"), "fr");
        assert_eq!(target_of("这是一段中文", "zh"), "en", "选的就是原文语言：退回默认");
        assert_eq!(target_of("This is an English sentence about the weather.", "en"), "zh");
    }

    /// 改动前 `translate_text` 里的提示词（目标写成 {T}）。默认方向的提示词一个字节都不能变：
    /// 它在真实模型上核对过，mock 也靠头一句认请求。
    const PROMPT_BEFORE: &str = "你是翻译引擎。把 user 给的整篇文本翻译成{T}。\n要求：\n1. 先通读全文再动笔，术语、人名、指代前后保持一致；\n2. 保留原有分段与换行；\n3. 代码、命令、路径、URL、版本号、数字原样保留；\n4. 只输出译文，不要解释、不要加引号或 Markdown 代码块。";

    #[test]
    fn prompt_is_byte_identical_for_the_default_directions() {
        for code in ["zh", "en"] {
            let target = lang::lang_by_code(code).expect("表里有");
            assert_eq!(system_prompt(target), PROMPT_BEFORE.replace("{T}", target.name), "{code}");
        }
    }

    #[test]
    fn prompt_names_the_chosen_language() {
        for target in lang::LANGS {
            let prompt = system_prompt(target);
            assert!(prompt.starts_with(&format!("你是翻译引擎。把 user 给的整篇文本翻译成{}。", target.name)), "{prompt}");
            // 其余几条要求不随语言变
            assert!(prompt.ends_with("4. 只输出译文，不要解释、不要加引号或 Markdown 代码块。"), "{}", target.code);
        }
        assert!(system_prompt(lang::lang_by_code("ja").unwrap()).contains("翻译成日语。"));
        assert!(system_prompt(lang::lang_by_code("zh-Hant").unwrap()).contains("翻译成繁体中文。"));
    }

    #[test]
    fn result_carries_both_languages_at_the_top_level() {
        let plan = lang::plan("こんにちは", "en");
        let value = serde_json::to_value(LongTextResult {
            text: "Hello".into(),
            truncated: false,
            incomplete: None,
            languages: Languages::of(&plan),
        })
        .expect("序列化");
        assert_eq!(value["text"], "Hello");
        assert_eq!(value["source"]["code"], "ja");
        assert_eq!(value["source_confident"], true);
        assert_eq!(value["target"]["code"], "en");

        let event = serde_json::to_value(LongTextLang { request_id: "r1".into(), languages: Languages::of(&plan) })
            .expect("序列化");
        assert_eq!(event["request_id"], "r1");
        assert_eq!(event["target"]["code"], "en");
    }
}
