//! OpenAI 兼容协议的客户端：一个 POST，一条 SSE 流。
//!
//! DeepSeek / Qwen / OpenRouter / Ollama 都吃这一套，差别只在 base_url 与 model。

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::providers::Provider;
use crate::stream::sse::SseDecoder;

/// 首字节超时：模型 8 秒不吐字就认为卡住。
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(8);
/// 整条流的兜底超时。
const STREAM_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone)]
pub struct ChatOutcome {
    pub text: String,
    pub first_delta_ms: u128,
    pub total_ms: u128,
    /// 结束原因。`length` 表示撞到 max_tokens，内容是被截断的。
    pub finish_reason: Option<String>,
    /// 收到的 SSE 分片数。用来区分「服务端只发了这么多」和「我们解析丢了」。
    pub chunks: usize,
}

#[derive(Debug, Clone)]
pub enum ChatError {
    /// Key 无效或过期（401/403）。不重试。
    Auth,
    /// 限流（429）。
    RateLimit,
    /// 请求被拒（400）：多半是我们塞的 reasoning 字段这家网关不认识。
    /// 单独一个变体，避免和「流里的 error 事件」混在一起误判。
    BadRequest(String),
    /// 服务端错误（5xx）或其它非 2xx。
    Server(String),
    /// 首字节超时。
    Timeout,
    /// 网络不可达。
    Network(String),
    /// 流里没有拿到任何内容。
    Empty,
}

impl ChatError {
    /// 直接可用的中文文案，对应方案 §2.6。
    pub fn message(&self) -> String {
        match self {
            ChatError::Auth => "Key 无效或已过期。检查一下，或换一个 provider。".to_string(),
            ChatError::RateLimit => "请求太频繁，歇几秒再试。".to_string(),
            ChatError::BadRequest(detail) => format!("请求被模型服务拒绝（HTTP 400）：{detail}"),
            ChatError::Server(detail) => format!("模型服务出错了：{detail}"),
            ChatError::Timeout => "模型 8 秒没回。检查网络，或换个模型。".to_string(),
            ChatError::Network(detail) => format!("连不上模型服务：{detail}"),
            ChatError::Empty => "模型返回了空内容。".to_string(),
        }
    }

    /// 只有限流和服务端错误值得重试。
    #[allow(dead_code)]
    pub fn retryable(&self) -> bool {
        matches!(self, ChatError::RateLimit | ChatError::Server(_) | ChatError::Timeout)
    }
}

/// 输出上限：五个义项 + 例句 + 备注，实测正常 1200–1800 token。
/// 给到 4096 与 deepseek-chat 的默认值持平——cap 比默认还低会截断，
/// 截断一次就得整条重来，那才是真的慢。
pub const MAX_TOKENS: u32 = 4096;

/// 组装请求体。`stream` 恒为 true：卡片靠增量渲染。
///
/// `reasoning` 是「关掉思考」的 provider 专属字段，直接并进顶层：
/// DeepSeek 的 `thinking`、DashScope 的 `enable_thinking` 都是顶层键。
pub fn build_body(
    model: &str,
    system: &str,
    user: &str,
    temperature: f64,
    reasoning: Option<&Value>,
    max_tokens: u32,
) -> Value {
    let mut body = json!({
        "model": model,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    });

    if let (Some(extra), Some(target)) = (reasoning, body.as_object_mut()) {
        if let Some(fields) = extra.as_object() {
            for (key, value) in fields {
                target.insert(key.clone(), value.clone());
            }
        }
    }

    body
}

/// 只认真正的 HTTP 400。流里那种 `{"error":...}` 事件不算——
/// 那时内容可能已经推给前端了，重试会把增量叠两遍。
pub fn is_bad_request(err: &ChatError) -> bool {
    matches!(err, ChatError::BadRequest(_))
}

/// 预热连接：只做 DNS + TCP + TLS 握手，不带 Key、不花 token。
///
/// 不带 Key 时 `/models` 会回 401，但握手已经完成；后续 POST 直接复用连接池，
/// 省掉首次查词 100–300ms 的建连时间。
pub async fn warmup(client: &reqwest::Client, provider: &Provider) {
    let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
    // 必须把 body 读完：HTTP/1.1 下没读完的响应不能回池，连接会被关掉，
    // 那就只剩 DNS 预热，等于没做。
    if let Ok(response) = client
        .get(&url)
        .timeout(Duration::from_secs(4))
        .send()
        .await
    {
        let _ = response.text().await;
    }
}

/// 发一次流式请求，边收边回调。
///
/// `on_delta` 收到的永远是「文本增量」，不是整包；解析失败的分片会被跳过
/// 而不是中断整条流——模型偶尔会吐一个坏 JSON，没必要为此丢掉全部内容。
pub async fn stream_chat(
    client: &reqwest::Client,
    provider: &Provider,
    api_key: Option<&str>,
    body: &Value,
    on_delta: &mut (dyn FnMut(&str) + Send),
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<ChatOutcome, ChatError> {
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    let mut request = client.post(&url).json(body);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }

    let started = Instant::now();
    let response = request.send().await.map_err(|err| {
        if err.is_timeout() {
            ChatError::Timeout
        } else {
            ChatError::Network(err.to_string())
        }
    })?;

    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(map_status(status.as_u16(), &detail));
    }

    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut text = String::new();
    let mut first_delta_ms: Option<u128> = None;
    let mut finish_reason: Option<String> = None;
    let mut chunks = 0usize;

    loop {
        let next = if first_delta_ms.is_none() {
            tokio::time::timeout(FIRST_BYTE_TIMEOUT, stream.next())
                .await
                .map_err(|_| ChatError::Timeout)?
        } else {
            tokio::time::timeout(STREAM_TIMEOUT, stream.next())
                .await
                .map_err(|_| ChatError::Timeout)?
        };

        // 用户点了「停止」：保留已经收到的部分，不再等后续分片
        if cancel.map(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) == Some(true) {
            break;
        }

        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|err| ChatError::Network(err.to_string()))?;

        for payload in decoder.push(&chunk) {
            chunks += 1;
            if payload.trim() == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };

            if let Some(reason) = value
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
            {
                finish_reason = Some(reason.to_string());
            }

            if let Some(message) = value.get("error").and_then(|item| item.get("message")) {
                return Err(ChatError::Server(message.as_str().map(sanitize_detail).unwrap_or_default()));
            }

            // 流式增量
            if let Some(delta) = value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
            {
                if delta.is_empty() {
                    continue;
                }
                first_delta_ms.get_or_insert_with(|| started.elapsed().as_millis());
                text.push_str(delta);
                on_delta(delta);
                continue;
            }

            // 少数 provider 忽略 stream，直接回整包
            if let Some(content) = value
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
            {
                if content.is_empty() {
                    continue;
                }
                first_delta_ms.get_or_insert_with(|| started.elapsed().as_millis());
                text.push_str(content);
                on_delta(content);
            }
        }
    }

    for payload in decoder.finish() {
        if let Ok(value) = serde_json::from_str::<Value>(&payload) {
            if let Some(delta) = value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
            {
                first_delta_ms.get_or_insert_with(|| started.elapsed().as_millis());
                text.push_str(delta);
                on_delta(delta);
            }
        }
    }

    if text.trim().is_empty() {
        return Err(ChatError::Empty);
    }

    Ok(ChatOutcome {
        text,
        first_delta_ms: first_delta_ms.unwrap_or(0),
        total_ms: started.elapsed().as_millis(),
        finish_reason,
        chunks,
    })
}

/// 把服务端返回的详情压成一行、限长：不把整页 HTML 塞进卡片。
fn sanitize_detail(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

fn map_status(status: u16, body: &str) -> ChatError {
    match status {
        400 => ChatError::BadRequest(sanitize_detail(body)),
        401 | 403 => ChatError::Auth,
        429 => ChatError::RateLimit,
        500..=599 => ChatError::Server(format!("HTTP {status}")),
        _ => {
            let detail = sanitize_detail(body);
            if detail.is_empty() {
                ChatError::Server(format!("HTTP {status}"))
            } else {
                ChatError::Server(format!("HTTP {status}：{}", detail.trim()))
            }
        }
    }
}

/// 测试连接：走 `/models`，不花 token，还能顺带拿到模型名与延迟。
pub async fn list_models(
    client: &reqwest::Client,
    provider: &Provider,
    api_key: Option<&str>,
) -> Result<(u128, Vec<String>), ChatError> {
    let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
    let mut request = client.get(&url);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }

    let started = Instant::now();
    let response = request
        .send()
        .await
        .map_err(|err| ChatError::Network(err.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(map_status(status.as_u16(), &detail));
    }

    let value: Value = response
        .json()
        .await
        .map_err(|err| ChatError::Network(format!("模型列表不是合法 JSON: {err}")))?;
    let models = value
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok((started.elapsed().as_millis(), models))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_is_streaming_and_carries_both_messages() {
        let body = build_body("m", "sys", "user", 0.3, None, MAX_TOKENS);
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["model"], json!("m"));
        assert_eq!(body["temperature"], json!(0.3));
        assert_eq!(body["max_tokens"], json!(4096));
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(body["messages"][1]["content"], json!("user"));
        assert!(body.get("thinking").is_none(), "没传就不该凭空出现");
    }

    #[test]
    fn reasoning_fields_are_merged_at_top_level() {
        let extra = json!({ "thinking": { "type": "disabled" } });
        let body = build_body("deepseek-flash", "sys", "user", 0.3, Some(&extra), MAX_TOKENS);
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert_eq!(body["model"], json!("deepseek-flash"), "不能把原有字段冲掉");
    }

    #[test]
    fn only_http_400_counts_as_bad_request() {
        assert!(is_bad_request(&map_status(400, "unknown field")));
        assert!(!is_bad_request(&map_status(422, "HTTP 400 in body")));
        assert!(!is_bad_request(&map_status(401, "")));
        assert!(!is_bad_request(&map_status(500, "")));
    }

    #[test]
    fn auth_errors_are_not_retryable() {
        assert!(!map_status(401, "").retryable());
        assert!(!map_status(403, "").retryable());
        assert!(map_status(429, "").retryable());
        assert!(map_status(503, "").retryable());
    }

    #[test]
    fn server_error_message_is_truncated() {
        let long = "x".repeat(500);
        match map_status(400, &long) {
            ChatError::BadRequest(detail) => {
                assert!(detail.chars().count() <= 200, "错误信息要截断")
            }
            other => panic!("应为 BadRequest，实际 {other:?}"),
        }
        match map_status(503, &long) {
            ChatError::Server(detail) => assert_eq!(detail, "HTTP 503", "5xx 不回传整页 HTML"),
            other => panic!("应为 Server，实际 {other:?}"),
        }
    }
}
