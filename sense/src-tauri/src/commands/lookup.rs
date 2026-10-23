//! `lookup` 命令：查一个词，流式吐回原始 JSON。
//!
//! 这里不解释模型输出、也不做字段校验——那是前端的 zod 的活。
//! Rust 只负责：取 Key、发请求、转发增量、落缓存。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::cache::{entry_key, Cache};
use crate::providers::openai_compat::{self, ChatError};
use crate::providers as provider_registry;
use crate::{prompts, secrets};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupRequest {
    pub word: String,
    pub lang: Option<String>,
    pub context: Option<String>,
    /// 前端生成，用来丢弃过期查询的增量。
    pub request_id: Option<String>,
    /// 第一次 0.3，校验失败重试时传 0.2。
    pub temperature: Option<f64>,
    /// 重试时跳过缓存：上一次的结果可能已经被写进库了。
    pub bypass_cache: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeltaEvent {
    pub request_id: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct LookupResult {
    pub raw: String,
    /// `cache` 或 `network`
    pub source: String,
    pub model: String,
    pub first_delta_ms: u128,
    pub total_ms: u128,
    /// 模型为什么停下：`stop` 正常、`length` 撞上限、`content_filter` 被内容策略中止
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LookupError {
    pub code: String,
    pub message: String,
}

impl LookupError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn from_chat(err: &ChatError) -> Self {
        let (code, message) = match err {
            ChatError::Auth => ("auth", err.message()),
            ChatError::RateLimit => ("rate_limit", err.message()),
            ChatError::Server(_) | ChatError::BadRequest(_) => ("server", err.message()),
            ChatError::Timeout => ("timeout", err.message()),
            ChatError::Network(_) => ("network", err.message()),
            ChatError::Empty => ("empty", err.message()),
        };
        Self::new(code, message)
    }
}

fn detect_lang(word: &str) -> &'static str {
    if word
        .chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
    {
        "zh"
    } else {
        "en"
    }
}

#[tauri::command]
pub async fn lookup(
    app: AppHandle,
    cache: State<'_, Cache>,
    req: LookupRequest,
) -> Result<LookupResult, LookupError> {
    let word = req.word.trim().to_string();
    if word.is_empty() {
        return Err(LookupError::new("empty_word", "先输入一个词。"));
    }
    if word.chars().count() > 64 {
        return Err(LookupError::new(
            "too_long",
            "这个词太长了，最多 64 个字符。",
        ));
    }

    let provider = provider_registry::active(&app)
        .map_err(|err| LookupError::new("no_provider", err))?;
    let lang = req.lang.unwrap_or_else(|| detect_lang(&word).to_string());

    let key = entry_key(
        &word,
        &lang,
        &provider.model,
        &provider.base_url,
        req.context.as_deref(),
        prompts::word_prompt_hash(),
    );

    // 命中缓存：直接返回，一个字节都不发出去。
    let bypass = req.bypass_cache.unwrap_or(false);
    if !bypass {
        if let Ok(Some(payload)) = cache.get(&key) {
            #[cfg(debug_assertions)]
            println!("[sense] cache hit: {word} ({lang})");
            return Ok(LookupResult {
                raw: payload,
                source: "cache".to_string(),
                model: provider.model,
                first_delta_ms: 0,
                total_ms: 0,
                finish_reason: Some("cache".to_string()),
            });
        }
    }

    let api_key = if provider.requires_key {
        match secrets::get_key(&provider.id) {
            Ok(Some(key)) => Some(key),
            Ok(None) => {
                return Err(LookupError::new(
                    "no_key",
                    "还没接入模型。去设置填一个 API Key，30 秒搞定。",
                ))
            }
            Err(err) => return Err(LookupError::new("keychain", err)),
        }
    } else {
        None
    };

    let system = prompts::word_system();
    let user = prompts::user_message(&word, &lang, req.context.as_deref());
    let temperature = req.temperature.unwrap_or(0.3);
    // 关掉思考：DeepSeek V4 默认开着 thinking（effort=high），
    // 一次查词光思维链就要几秒，卡片等于转圈。
    let reasoning = provider_registry::reasoning_fields(&provider.id);
    let body = openai_compat::build_body(
        &provider.model,
        system,
        &user,
        temperature,
        reasoning.as_ref(),
        openai_compat::MAX_TOKENS,
    );
    let plain_body = || {
        openai_compat::build_body(
            &provider.model,
            system,
            &user,
            temperature,
            None,
            openai_compat::MAX_TOKENS,
        )
    };

    let request_id = req
        .request_id
        .unwrap_or_else(|| format!("req-{}", std::process::id()));

    let app_for_emit = app.clone();
    let request_for_emit = request_id.clone();
    // 只要往前端推过一个字节，就不能再「去掉 reasoning 字段重发」——
    // 重发的增量会接在同一段 buffer 后面，卡片里会出现两份义项。
    let emitted = Arc::new(AtomicBool::new(false));
    let emitted_for_delta = emitted.clone();
    let mut on_delta = move |chunk: &str| {
        emitted_for_delta.store(true, Ordering::Relaxed);
        let _ = app_for_emit.emit(
            "sense://delta",
            DeltaEvent {
                request_id: request_for_emit.clone(),
                text: chunk.to_string(),
            },
        );
    };

    let client = crate::client_for(&provider.base_url);
    let outcome = match openai_compat::stream_chat(
        client,
        &provider,
        api_key.as_deref(),
        &body,
        &mut on_delta,
        None,
    )
    .await
    {
        Ok(outcome) => outcome,
        // 网关不认 reasoning 字段：去掉再来一次，别让用户看到 400。
        // 前提是从没往前端吐过字，否则会把内容叠两遍。
        Err(err)
            if reasoning.is_some()
                && !emitted.load(Ordering::Relaxed)
                && openai_compat::is_bad_request(&err) =>
        {
            eprintln!("[sense] provider 不认关思考字段，去掉后重试");
            openai_compat::stream_chat(
                client,
                &provider,
                api_key.as_deref(),
                &plain_body(),
                &mut on_delta,
                None,
            )
            .await
            .map_err(|err| LookupError::from_chat(&err))?
        }
        Err(err) => return Err(LookupError::from_chat(&err)),
    };

    println!(
        "[sense] lookup {word}: ttft={}ms total={}ms chars={} thinking={} finish={:?}",
        outcome.first_delta_ms,
        outcome.total_ms,
        outcome.text.chars().count(),
        if reasoning.is_some() { "off" } else { "n/a" },
        outcome.finish_reason
    );

    // 只缓存「结构完整」的输出：半截 JSON 进了库，下次查询会拿着坏数据反复失败。
    if looks_complete(&outcome.text) {
        if let Err(err) = cache.put(&key, &word, &lang, &provider.model, &outcome.text) {
            // 缓存写失败不该让用户拿不到结果，只记日志。
            eprintln!("[sense] {err}");
        }
    } else {
        // 记下长度与结尾：判断是「模型被中止」还是「解析丢了尾巴」
        let tail: String = outcome
            .text
            .chars()
            .rev()
            .take(80)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        eprintln!(
            "[sense] 输出结构不完整 len={} finish={:?} tail={:?}",
            outcome.text.chars().count(),
            outcome.finish_reason,
            tail
        );
    }

    Ok(LookupResult {
        raw: outcome.text,
        source: "network".to_string(),
        model: provider.model,
        first_delta_ms: outcome.first_delta_ms,
        total_ms: outcome.total_ms,
        finish_reason: outcome.finish_reason,
    })
}

/// Rust 侧只做最廉价的结构门：能解析成对象、九个顶层字段齐全、senses 非空。
/// 语义校验仍然是前端 zod 的事。
fn looks_complete(raw: &str) -> bool {
    const REQUIRED: [&str; 9] = [
        "schema_version",
        "word",
        "lang",
        "pronunciation",
        "pos",
        "frequency",
        "senses",
        "confusables",
        "notes",
    ];
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    if !REQUIRED.iter().all(|key| map.contains_key(*key)) {
        return false;
    }
    map.get("senses")
        .and_then(|item| item.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false)
}

/// 前端校验失败时调用：把这条缓存标成过期，避免下次又读到坏数据。
#[tauri::command]
pub fn invalidate_lookup(
    app: AppHandle,
    cache: State<'_, Cache>,
    word: String,
    lang: String,
    context: Option<String>,
) -> Result<(), String> {
    let provider = provider_registry::active(&app)?;
    let key = entry_key(
        &word,
        &lang,
        &provider.model,
        &provider.base_url,
        context.as_deref(),
        prompts::word_prompt_hash(),
    );
    cache.mark_stale(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_language_from_characters() {
        assert_eq!(detect_lang("charge"), "en");
        assert_eq!(detect_lang("落实"), "zh");
        assert_eq!(detect_lang("AI 助手"), "zh");
    }

    #[test]
    fn structural_gate_rejects_truncated_output() {
        assert!(!looks_complete(""));
        assert!(!looks_complete("{\"word\":\"charge\""));
        assert!(!looks_complete(
            r#"{"schema_version":1,"word":"charge","lang":"en","pronunciation":{},"pos":[],"frequency":{},"senses":[],"confusables":[],"notes":null}"#
        ), "空 senses 不算完整");

        let full = r#"{"schema_version":1,"word":"charge","lang":"en","pronunciation":{"ipa":null,"pinyin":null},"pos":[],"frequency":{"zipf":null,"band":null},"senses":[{"rank":1}],"confusables":[],"notes":null}"#;
        assert!(looks_complete(full));
    }

    #[test]
    fn chat_errors_map_to_codes() {
        assert_eq!(LookupError::from_chat(&ChatError::Auth).code, "auth");
        assert_eq!(
            LookupError::from_chat(&ChatError::RateLimit).code,
            "rate_limit"
        );
        assert_eq!(LookupError::from_chat(&ChatError::Timeout).code, "timeout");
    }
}
