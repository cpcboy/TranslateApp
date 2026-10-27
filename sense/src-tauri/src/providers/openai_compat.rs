//! OpenAI 兼容协议的客户端：一个 POST，一条 SSE 流。
//!
//! DeepSeek / Qwen / OpenRouter / Ollama 都吃这一套，差别只在 base_url 与 model。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::providers::Provider;
use crate::stream::sse::SseDecoder;

/// 首字节超时：开始收流后 8 秒还没吐出第一段内容，就认为卡住。
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(8);
/// 分片之间的空闲超时：流已经开始后，连续这么久没有新分片就认为卡住。
/// 它不是整条流的时限，整条流的上限见 `stream_deadline`。
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// 等服务端开始响应（返回响应头）的时限，和以前客户端上的总时限一致。
const HEADERS_TIMEOUT: Duration = Duration::from_secs(180);

/// 整条流式请求的时限。
///
/// 不能沿用客户端上的 180 秒总时限：reqwest 的 `timeout` 管到响应体读完，
/// 长文 8192 token 按 30–60 token/s 要生成 140–270 秒，会被拦腰掐断。
/// 查词、截屏、语法、划词（max_tokens ≤ 4096）仍是 180 秒，与以前一致；
/// 只有更长的输出按最慢 10 token/s 估算、再留 60 秒。卡住的流由首字节与空闲超时兜底。
pub fn stream_deadline(max_tokens: u64) -> Duration {
    if max_tokens <= u64::from(MAX_TOKENS) {
        Duration::from_secs(180)
    } else {
        Duration::from_secs(max_tokens / 10 + 60)
    }
}

/// 取消检查：返回 true 表示用户点了「停止」。每收到一片就问一次。
pub type CancelCheck<'a> = &'a (dyn Fn() -> bool + Sync);

#[derive(Debug, Clone)]
pub struct ChatOutcome {
    pub text: String,
    pub first_delta_ms: u128,
    pub total_ms: u128,
    /// 结束原因。`length` 表示撞到 max_tokens，内容是被截断的。
    pub finish_reason: Option<String>,
    /// 收到的 SSE 分片数。用来区分「服务端只发了这么多」和「我们解析丢了」。
    pub chunks: usize,
    /// 收到了 `[DONE]`。没有 finish_reason 也没有它，说明连接是提前关的。
    pub done: bool,
    /// 因为用户点了「停止」才结束。
    pub cancelled: bool,
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
    /// 超时。附带是哪一段超时，直接进文案（以前一律写「8 秒没回」，连接超时也这么说）。
    Timeout(String),
    /// 网络不可达（还没开始收流）。
    Network(String),
    /// 已经开始收流、中途断开。和「连不上」分开：长文据此提示「译文不完整」。
    /// 底层原因在出错处直接打进日志，这里不再携带。
    Interrupted,
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
            ChatError::Timeout(stage) => format!("模型响应超时：{stage}。检查网络，或换个模型。"),
            ChatError::Network(detail) => format!("连不上模型服务：{detail}"),
            // 底层原因（hyper 的英文报错）只进日志，用户看不懂也帮不上忙
            ChatError::Interrupted => {
                "传输中途断开，连接在回复写完前断了（可能是网络抖动或服务端掐断）".to_string()
            }
            ChatError::Empty => "模型返回了空内容。".to_string(),
        }
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
    cancel: Option<CancelCheck<'_>>,
) -> Result<ChatOutcome, ChatError> {
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    // 按本次的 max_tokens 单独设时限，覆盖客户端上那个只适合非流式请求的 180 秒
    let max_tokens = body
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(MAX_TOKENS));
    let deadline = stream_deadline(max_tokens);
    let mut request = client.post(&url).json(body).timeout(deadline);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }

    let started = Instant::now();
    // 等响应头单独限 180 秒：长文的整条时限放宽到十几分钟，不能让「服务端一直不响应」也等那么久
    let response = tokio::time::timeout(HEADERS_TIMEOUT, request.send())
        .await
        .map_err(|_| {
            ChatError::Timeout(format!("{} 秒内服务端没有开始响应", HEADERS_TIMEOUT.as_secs()))
        })?
        .map_err(|err| {
            if err.is_timeout() {
                ChatError::Timeout("建立连接或等待服务端响应超时".to_string())
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
    let mut done = false;
    let mut cancelled = false;

    loop {
        let next = if first_delta_ms.is_none() {
            tokio::time::timeout(FIRST_BYTE_TIMEOUT, stream.next())
                .await
                .map_err(|_| {
                    ChatError::Timeout(format!(
                        "{} 秒内没有收到第一段内容",
                        FIRST_BYTE_TIMEOUT.as_secs()
                    ))
                })?
        } else {
            tokio::time::timeout(IDLE_TIMEOUT, stream.next())
                .await
                .map_err(|_| {
                    ChatError::Timeout(format!("连续 {} 秒没有新内容", IDLE_TIMEOUT.as_secs()))
                })?
        };

        // 用户点了「停止」：保留已经收到的部分，不再等后续分片
        if cancel.is_some_and(|stopped| stopped()) {
            cancelled = true;
            break;
        }

        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|err| {
            if err.is_timeout() {
                ChatError::Timeout(format!("整条回复超过 {} 秒", deadline.as_secs()))
            } else {
                // 已经开始收流，这时出错不是「连不上」，是中途断开；原始原因留在日志里
                eprintln!("[sense] 流中途断开：{err:?}");
                ChatError::Interrupted
            }
        })?;

        for payload in decoder.push(&chunk) {
            chunks += 1;
            if payload.trim() == "[DONE]" {
                done = true;
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
        if payload.trim() == "[DONE]" {
            done = true;
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&payload) {
            if let Some(reason) = value
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
            {
                finish_reason = Some(reason.to_string());
            }
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

    // 用户在第一个字之前就点了停止：这是「已停止」，不是「模型返回了空内容」
    if text.trim().is_empty() && !cancelled {
        // 一个字都没有，但服务端给了结束原因（比如 content_filter）：把原因说出来
        return Err(match finish_reason.as_deref() {
            Some(reason) if reason != "stop" => {
                ChatError::Server(format!("没有输出任何内容就结束了（{reason}）"))
            }
            _ => ChatError::Empty,
        });
    }

    Ok(ChatOutcome {
        text,
        first_delta_ms: first_delta_ms.unwrap_or(0),
        total_ms: started.elapsed().as_millis(),
        finish_reason,
        chunks,
        done,
        cancelled,
    })
}

/// 记住「这家网关不认关思考字段」的 (base_url, model)：之后直接不带，省掉每次一个 400 的往返。
/// 只活在本次运行里——网关升级后重启应用就会重新试一次。
static NO_REASONING: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn reasoning_key(provider: &Provider) -> String {
    format!("{}|{}", provider.base_url, provider.model)
}

fn reasoning_rejected_before(provider: &Provider) -> bool {
    let key = reasoning_key(provider);
    NO_REASONING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&key)
}

fn remember_reasoning_rejected(provider: &Provider) {
    let key = reasoning_key(provider);
    let mut known = NO_REASONING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !known.contains(&key) {
        known.push(key);
    }
}

/// 把 body 顶层里「关思考」的那几个键拿掉。
fn without_reasoning(body: &Value, reasoning: &Value) -> Value {
    let mut plain = body.clone();
    if let (Some(target), Some(fields)) = (plain.as_object_mut(), reasoning.as_object()) {
        for key in fields.keys() {
            target.remove(key);
        }
    }
    plain
}

/// `stream_chat` + 「关思考」字段兜底，查词、长文、语法、划词、截屏都走这里。
///
/// 有的网关不认 `thinking` / `enable_thinking` / `reasoning` 之类的字段，会回 400。
/// 这时去掉这些字段重发一次，而不是让用户看到 400。前提：
/// - 只认真正的 HTTP 400。它在任何响应体之前到达，所以重发不会和已经推出去的增量叠成两份
///   （流里的 error 事件是 `Server`，不会走到这里）；
/// - 用户没在等待期间点「停止」；
/// - 重发成功才记住「这家不认」。重发还是 400，说明问题不在这个字段上，下次照旧带上。
///
/// 已知不覆盖：回 422、或者在 200 的流里报「不认字段」的网关；以及与思考字段无关的 400
/// （比如上下文太长）也会白白重发一次。
pub async fn stream_chat_adaptive(
    client: &reqwest::Client,
    provider: &Provider,
    api_key: Option<&str>,
    body: &Value,
    reasoning: Option<&Value>,
    on_delta: &mut (dyn FnMut(&str) + Send),
    cancel: Option<CancelCheck<'_>>,
) -> Result<ChatOutcome, ChatError> {
    let Some(extra) = reasoning else {
        return stream_chat(client, provider, api_key, body, on_delta, cancel).await;
    };

    if reasoning_rejected_before(provider) {
        let plain = without_reasoning(body, extra);
        return stream_chat(client, provider, api_key, &plain, on_delta, cancel).await;
    }

    let first = stream_chat(client, provider, api_key, body, on_delta, cancel).await;
    match first {
        // 用户在这一来一回的 400 期间点了停止：不重发，也不报「请求被拒绝」，按「已停止」返回（和 stream_chat 的约定一致）
        Err(err) if is_bad_request(&err) && cancel.is_some_and(|stopped| stopped()) => Ok(ChatOutcome {
            text: String::new(),
            first_delta_ms: 0,
            total_ms: 0,
            finish_reason: None,
            chunks: 0,
            done: false,
            cancelled: true,
        }),
        Err(err) if is_bad_request(&err) => {
            eprintln!("[sense] provider 不认关思考字段，去掉后重试");
            let plain = without_reasoning(body, extra);
            let retried = stream_chat(client, provider, api_key, &plain, on_delta, cancel).await;
            if retried.is_ok() {
                remember_reasoning_rejected(provider);
            }
            retried
        }
        other => other,
    }
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
    fn statuses_map_to_the_right_errors() {
        assert!(matches!(map_status(401, ""), ChatError::Auth));
        assert!(matches!(map_status(403, ""), ChatError::Auth));
        assert!(matches!(map_status(429, ""), ChatError::RateLimit));
        assert!(matches!(map_status(503, ""), ChatError::Server(_)));
    }

    #[test]
    fn stream_deadline_only_extends_long_outputs() {
        assert_eq!(stream_deadline(600), Duration::from_secs(180), "划词与以前一致");
        assert_eq!(stream_deadline(4096), Duration::from_secs(180), "查词、截屏与以前一致");
        assert_eq!(stream_deadline(8192), Duration::from_secs(879), "长文要给足十几分钟");
    }

    fn test_provider(base_url: &str) -> Provider {
        Provider {
            id: "test".into(),
            label: "test".into(),
            base_url: base_url.into(),
            model: "m".into(),
            requires_key: false,
        }
    }

    /// 把请求读完整（头 + Content-Length 指定的 body），否则关连接时会发 RST，测试会变得不稳。
    /// 返回整个请求的文本，便于断言发了什么。
    fn read_request_text(socket: &mut std::net::TcpStream) -> String {
        use std::io::Read;
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let read = socket.read(&mut buf).unwrap_or(0);
            if read == 0 {
                break;
            }
            data.extend_from_slice(&buf[..read]);
            let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                break;
            }
        }
        String::from_utf8_lossy(&data).into_owned()
    }

    fn read_request(socket: &mut std::net::TcpStream) {
        let _ = read_request_text(socket);
    }

    /// 只服务一次的本地 SSE 服务（chunked 编码），按给定间隔逐条写出增量；
    /// 事件写成 `[DONE]` 时原样发出结束标记。
    /// `finish` 为 false 时不写结束分片就断开，模拟流被中途掐断。
    fn serve_sse(events: Vec<&'static str>, gap: Duration, finish: bool) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定端口");
        let port = listener.local_addr().expect("取端口").port();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("接受连接");
            read_request(&mut socket);
            let _ = socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
            );
            for event in events {
                let payload = if event == "[DONE]" {
                    "data: [DONE]\n\n".to_string()
                } else {
                    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{event}\"}}}}]}}\n\n")
                };
                let _ = socket.write_all(format!("{:x}\r\n{payload}\r\n", payload.len()).as_bytes());
                let _ = socket.flush();
                std::thread::sleep(gap);
            }
            if finish {
                let _ = socket.write_all(b"0\r\n\r\n");
            }
        });
        format!("http://127.0.0.1:{port}/v1")
    }

    #[tokio::test]
    async fn stream_is_not_cut_by_client_total_timeout() {
        // 客户端总时限 400ms，整条流要 ~900ms：修之前就是在这里被拦腰掐断
        let base_url = serve_sse(vec!["a", "b", "c", "d", "e", "f"], Duration::from_millis(150), true);
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(400))
            .build()
            .expect("构建客户端");
        let body = build_body("m", "sys", "user", 0.3, None, 100);
        let mut seen = String::new();
        let outcome = stream_chat(
            &client,
            &test_provider(&base_url),
            None,
            &body,
            &mut |delta: &str| seen.push_str(delta),
            None,
        )
        .await
        .expect("流式请求要用自己的时限，不能被客户端的总时限掐断");
        assert_eq!(outcome.text, "abcdef");
        assert_eq!(seen, "abcdef");
        assert!(!outcome.done, "没发 [DONE] 就不能算收到了结束标记");
        assert!(!outcome.cancelled);
    }

    #[tokio::test]
    async fn done_marker_is_recorded() {
        let base_url = serve_sse(vec!["a", "[DONE]"], Duration::from_millis(10), true);
        let client = reqwest::Client::builder().no_proxy().build().expect("构建客户端");
        let body = build_body("m", "sys", "user", 0.3, None, 100);
        let outcome = stream_chat(
            &client,
            &test_provider(&base_url),
            None,
            &body,
            &mut |_delta: &str| {},
            None,
        )
        .await
        .expect("正常结束");
        assert!(outcome.done);
        assert_eq!(outcome.text, "a");
    }

    #[tokio::test]
    async fn dropped_stream_is_interrupted_not_unreachable() {
        let base_url = serve_sse(vec!["a", "b"], Duration::from_millis(20), false);
        let client = reqwest::Client::builder().no_proxy().build().expect("构建客户端");
        let body = build_body("m", "sys", "user", 0.3, None, 100);
        let mut seen = String::new();
        let err = stream_chat(
            &client,
            &test_provider(&base_url),
            None,
            &body,
            &mut |delta: &str| seen.push_str(delta),
            None,
        )
        .await
        .expect_err("中途断开必须报错，不能当成正常结束");
        assert!(matches!(err, ChatError::Interrupted), "应为 Interrupted，实际 {err:?}");
        assert!(err.message().starts_with("传输中途断开"), "{}", err.message());
        assert_eq!(seen, "ab", "断开前收到的增量照常推给前端");
    }

    /// 每个连接都读完整个请求并记下来；请求体里有 `reject` 就回 400，否则回一条正常的 SSE。
    /// 全部回应都带 `Connection: close`，保证重发走新连接，服务端好区分。
    fn serve_rejecting(reject: &'static str, connections: usize) -> (String, std::sync::Arc<Mutex<Vec<String>>>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定端口");
        let port = listener.local_addr().expect("取端口").port();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        std::thread::spawn(move || {
            for _ in 0..connections {
                let Ok((mut socket, _)) = listener.accept() else { return };
                let request = read_request_text(&mut socket);
                let rejected = request.contains(reject);
                recorded.lock().unwrap().push(request);
                if rejected {
                    let body = r#"{"error":{"message":"unknown field"}}"#;
                    let _ = socket.write_all(
                        format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    );
                } else {
                    let payload = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
                    let _ = socket.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
                            payload.len()
                        )
                        .as_bytes(),
                    );
                }
                let _ = socket.flush();
            }
        });
        (format!("http://127.0.0.1:{port}/v1"), seen)
    }

    fn thinking_off() -> Value {
        json!({ "thinking": { "type": "disabled" } })
    }

    async fn ask_adaptive(base_url: &str) -> Result<ChatOutcome, ChatError> {
        let client = reqwest::Client::builder().no_proxy().build().expect("构建客户端");
        let extra = thinking_off();
        let body = build_body("m", "sys", "user", 0.3, Some(&extra), 100);
        stream_chat_adaptive(
            &client,
            &test_provider(base_url),
            None,
            &body,
            Some(&extra),
            &mut |_delta: &str| {},
            None,
        )
        .await
    }

    #[tokio::test]
    async fn rejected_reasoning_field_is_dropped_and_remembered() {
        let (base_url, seen) = serve_rejecting("thinking", 3);

        // 第一次：带字段 → 400 → 去掉字段重发 → 成功
        let outcome = ask_adaptive(&base_url).await.expect("去掉字段后应该成功");
        assert_eq!(outcome.text, "ok");
        {
            let requests = seen.lock().unwrap();
            assert_eq!(requests.len(), 2, "400 之后重发一次");
            assert!(requests[0].contains("thinking"));
            assert!(!requests[1].contains("thinking"), "重发不能再带关思考字段");
            assert!(requests[1].contains("\"max_tokens\":100"), "其余字段原样保留");
        }

        // 第二次：已经记住这家不认，直接不带，不再多吃一个 400
        ask_adaptive(&base_url).await.expect("第二次直接成功");
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3, "第二次只发了一个请求");
        assert!(!requests[2].contains("thinking"));
    }

    #[tokio::test]
    async fn a_400_that_is_not_about_reasoning_is_not_remembered() {
        // 服务端对什么请求都回 400（rejected 命中 "model"）：问题不在 thinking 上
        let (base_url, seen) = serve_rejecting("model", 4);

        let err = ask_adaptive(&base_url).await.expect_err("两次都是 400");
        assert!(matches!(err, ChatError::BadRequest(_)), "实际 {err:?}");
        assert_eq!(seen.lock().unwrap().len(), 2);

        // 重发也失败，不能记成「这家不认关思考」：下次照旧带上字段
        ask_adaptive(&base_url).await.expect_err("仍然是 400");
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests[2].contains("thinking"), "没被记住，第三个请求应当仍带字段");
    }

    #[test]
    fn without_reasoning_only_removes_those_keys() {
        let extra = thinking_off();
        let body = build_body("m", "sys", "user", 0.3, Some(&extra), 100);
        let plain = without_reasoning(&body, &extra);
        assert!(plain.get("thinking").is_none());
        assert_eq!(plain["model"], json!("m"));
        assert_eq!(plain["messages"], body["messages"]);
        assert_eq!(body["thinking"]["type"], json!("disabled"), "原 body 不被改动");
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
