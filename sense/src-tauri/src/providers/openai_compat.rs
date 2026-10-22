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
}

#[derive(Debug, Clone)]
pub enum ChatError {
    /// Key 无效或过期（401/403）。不重试。
    Auth,
    /// 限流（429）。
    RateLimit,
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

/// 组装请求体。`stream` 恒为 true：卡片靠增量渲染。
pub fn build_body(model: &str, system: &str, user: &str, temperature: f64) -> Value {
    json!({
        "model": model,
        "temperature": temperature,
        "stream": true,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    })
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

        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|err| ChatError::Network(err.to_string()))?;

        for payload in decoder.push(&chunk) {
            if payload.trim() == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };

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
        let body = build_body("m", "sys", "user", 0.3);
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["model"], json!("m"));
        assert_eq!(body["temperature"], json!(0.3));
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(body["messages"][1]["content"], json!("user"));
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
            ChatError::Server(detail) => assert!(detail.chars().count() <= 200, "错误信息要截断"),
            other => panic!("应为 Server，实际 {other:?}"),
        }
    }
}
