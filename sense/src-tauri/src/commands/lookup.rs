//! `lookup` 命令：查一个词，流式吐回原始 JSON。
//!
//! 这里不解释模型输出、也不做字段校验——那是前端的 zod 的活。
//! Rust 只负责：取 Key、发请求、转发增量、落缓存。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

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
            ChatError::Timeout(_) => ("timeout", err.message()),
            ChatError::Network(_) => ("network", err.message()),
            // 单独一个 code：卡片标题不能写「连不上」，明明已经连上、只是中途断了
            ChatError::Interrupted => ("interrupted", err.message()),
            ChatError::Empty => ("empty", err.message()),
        };
        Self::new(code, message)
    }
}

/// 发给模型的两条消息：system 按输入语言取提示词（中文输入追加「先译成英文」一节），user 是一行 JSON。
/// 单独成函数，是为了让「中文输入真的发了中文提示词」这条接线能被单测盯住。
fn lookup_messages(word: &str, lang: &str, context: Option<&str>) -> (&'static str, String) {
    (
        prompts::word_system(lang),
        prompts::user_message(word, lang, context),
    )
}

/// 查词缓存的 key。`lookup` 写缓存、`invalidate_lookup` 标过期必须算出同一个 key，
/// 所以只此一处：提示词指纹按输入语言取（中文输入的提示词和英文的不同）。
fn lookup_key(word: &str, lang: &str, provider: &provider_registry::Provider, context: Option<&str>) -> String {
    entry_key(
        word,
        lang,
        &provider.model,
        &provider.base_url,
        context,
        prompts::word_prompt_hash(lang),
    )
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

    let key = lookup_key(&word, &lang, &provider, req.context.as_deref());

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

    // 中文输入：提示词要求先译成英文、再解析那个英文词（词条里的 word 是英文）
    let (system, user) = lookup_messages(&word, &lang, req.context.as_deref());
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
    let request_id = req
        .request_id
        .unwrap_or_else(|| format!("req-{}", std::process::id()));

    let app_for_emit = app.clone();
    let request_for_emit = request_id.clone();
    let mut on_delta = move |chunk: &str| {
        let _ = app_for_emit.emit(
            "sense://delta",
            DeltaEvent {
                request_id: request_for_emit.clone(),
                text: chunk.to_string(),
            },
        );
    };

    // 网关不认关思考字段（400）时，公共层会去掉字段重发，并记住这家不认
    let client = crate::client_for(&provider.base_url);
    let outcome = openai_compat::stream_chat_adaptive(
        client,
        &provider,
        api_key.as_deref(),
        &body,
        reasoning.as_ref(),
        &mut on_delta,
        None,
    )
    .await
    .map_err(|err| LookupError::from_chat(&err))?;

    // 查过的词是用户内容：只在 debug 构建里打印，发布版的终端 / 日志里不留
    #[cfg(debug_assertions)]
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
        // 记下长度与结束原因：判断是「模型被中止」还是「解析丢了尾巴」。
        // 词和输出结尾是用户内容，只有 debug 构建才写进去；发布版只留这些不含内容的数字
        let line = incomplete_log_line(
            &word,
            &outcome.text,
            outcome.chunks,
            outcome.finish_reason.as_deref(),
            cfg!(debug_assertions),
        );
        eprintln!("[sense] 输出结构不完整 {line}");
        // 从访达启动时 stdout 看不到，落一份文件
        if let Ok(dir) = app.path().app_data_dir() {
            use std::io::Write;
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("lookup.log"))
            {
                let _ = writeln!(file, "{line}");
            }
        }
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

/// 输出不完整时写进 lookup.log 的那一行。`with_content` 为 false 时不含词和输出结尾。
fn incomplete_log_line(
    word: &str,
    text: &str,
    chunks: usize,
    finish_reason: Option<&str>,
    with_content: bool,
) -> String {
    let len = text.chars().count();
    if !with_content {
        return format!("lookup: len={len} chunks={chunks} finish={finish_reason:?}");
    }
    let tail: String = text.chars().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect();
    format!("lookup {word}: len={len} chunks={chunks} finish={finish_reason:?} tail={tail:?}")
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
    invalidate(&cache, &provider, &word, &lang, context.as_deref())
}

/// 把一条查词缓存标成过期。单独成函数，key 的算法（必须与写缓存时同一个）才能被单测盯住。
fn invalidate(
    cache: &Cache,
    provider: &provider_registry::Provider,
    word: &str,
    lang: &str,
    context: Option<&str>,
) -> Result<(), String> {
    cache.mark_stale(&lookup_key(word, lang, provider, context))
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

    fn provider() -> provider_registry::Provider {
        provider_registry::Provider {
            id: "p".into(),
            label: "P".into(),
            base_url: "https://a/v1".into(),
            model: "m".into(),
            requires_key: false,
        }
    }

    #[test]
    fn cache_key_follows_the_prompt_of_the_input_language() {
        let p = provider();
        // 同一个词，中文输入和英文输入用的提示词不同，key 必须不同
        assert_ne!(
            lookup_key("charge", "en", &p, None),
            lookup_key("charge", "zh", &p, None)
        );
        // 英文输入的 key 与改动前一致：就是用英文提示词的指纹算的，已缓存的英文词条继续有效
        assert_eq!(
            lookup_key("charge", "en", &p, None),
            entry_key("charge", "en", "m", "https://a/v1", None, prompts::word_prompt_hash("en"))
        );
        assert_eq!(
            lookup_key("充电", "zh", &p, None),
            entry_key("充电", "zh", "m", "https://a/v1", None, prompts::word_prompt_hash("zh"))
        );
        // 带上下文的另算
        assert_ne!(
            lookup_key("charge", "en", &p, Some("ctx")),
            lookup_key("charge", "en", &p, None)
        );
    }

    #[test]
    fn request_messages_carry_the_prompt_of_the_input_language() {
        // 中文输入：system 带「先译成英文」那一节，user 里 lang 是 zh
        let (system, user) = lookup_messages("充电", "zh", None);
        assert!(system.contains("# 中文输入"), "中文输入必须发中文那一节");
        let value: serde_json::Value = serde_json::from_str(&user).expect("user 必须是合法 JSON");
        assert_eq!(value["word"], "充电");
        assert_eq!(value["lang"], "zh");

        // 英文输入：原提示词，不带那一节
        let (system, user) = lookup_messages("charge", "en", None);
        assert!(!system.contains("# 中文输入"), "英文输入不能带中文那一节");
        let value: serde_json::Value = serde_json::from_str(&user).expect("user 必须是合法 JSON");
        assert_eq!(value["lang"], "en");
    }

    #[test]
    fn invalidate_hits_the_same_row_that_lookup_wrote() {
        let cache = Cache::in_memory().expect("内存库");
        let p = provider();
        // lookup 写缓存用的 key：中文输入、英文输入各一条
        let zh_key = lookup_key("充电", "zh", &p, None);
        let en_key = lookup_key("charge", "en", &p, None);
        cache.put(&zh_key, "充电", "zh", "m", "{\"zh\":1}").expect("写");
        cache.put(&en_key, "charge", "en", "m", "{\"en\":1}").expect("写");

        invalidate(&cache, &p, "充电", "zh", None).expect("标过期");

        assert!(cache.get(&zh_key).expect("查").is_none(), "中文那条必须被标废");
        assert!(cache.get(&en_key).expect("查").is_some(), "英文那条不能被牵连");

        // 语言写错就标不到：key 里的提示词指纹随语言变
        let other = lookup_key("充电", "en", &p, None);
        cache.put(&other, "充电", "en", "m", "{\"x\":1}").expect("写");
        invalidate(&cache, &p, "充电", "zh", None).expect("标过期");
        assert!(cache.get(&other).expect("查").is_some(), "按 zh 标废不该动 en 的 key");
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
    fn release_log_line_carries_no_user_content() {
        let line = incomplete_log_line("秘密词", "模型输出的结尾内容", 7, Some("length"), false);
        assert!(!line.contains("秘密词") && !line.contains("结尾内容"), "{line}");
        assert!(line.contains("len=") && line.contains("length"), "{line}");

        let debug = incomplete_log_line("秘密词", "模型输出的结尾内容", 7, Some("length"), true);
        assert!(debug.contains("秘密词") && debug.contains("结尾内容"), "{debug}");
    }

    #[test]
    fn chat_errors_map_to_codes() {
        assert_eq!(LookupError::from_chat(&ChatError::Auth).code, "auth");
        assert_eq!(
            LookupError::from_chat(&ChatError::RateLimit).code,
            "rate_limit"
        );
        assert_eq!(
            LookupError::from_chat(&ChatError::Timeout("连续 90 秒没有新内容".into())).code,
            "timeout"
        );
        assert_eq!(
            LookupError::from_chat(&ChatError::Interrupted).code,
            "interrupted"
        );
    }
}
