//! OpenAI 兼容协议的客户端：一个 POST，一条 SSE 流。
//!
//! DeepSeek / Qwen / OpenRouter / Ollama 都吃这一套，差别只在 base_url 与 model。

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::providers::Provider;
use crate::stream::sse::SseDecoder;

/// 首字节超时：开始收流后 8 秒还没吐出第一段内容，就认为卡住。
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(8);
/// 分片之间的空闲超时：流已经开始后，连续这么久没有新分片就认为卡住。
/// 它不是整条流的时限，整条流的上限见 `stream_deadline`。
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// 本地模型等响应头的时限（Ollama 冷加载大模型可能超过一两分钟）。
const HEADERS_TIMEOUT: Duration = Duration::from_secs(180);
/// 云端等响应头的单一时限：覆盖正常网络波动与较长 prefill/排队；不再用 8 秒短超时重发非幂等 POST。
const CLOUD_HEADERS_TIMEOUT: Duration = Duration::from_secs(45);
/// 连接池空闲淘汰窗口（20s）加 5s 余量：若系统刚休眠唤醒且仍落在这个窗口内，或预热失败，直接绕过连接池改走新连接。
const POOL_BYPASS_WINDOW: Duration = Duration::from_secs(25);
/// `Instant`（macOS `CLOCK_UPTIME_RAW`，休眠停表）与 `SystemTime` 差值超过 5 秒即判定经历过系统休眠。
const SLEEP_WALL_DRIFT: Duration = Duration::from_secs(5);
const POOL_HEALTH_TTL: Duration = Duration::from_secs(60 * 60);
/// `NO_REASONING` 缓存条目有效期：避免某次异常 400 或网关临时波动让多日不关机的进程永久丢掉关思考参数。
const NO_REASONING_TTL: Duration = Duration::from_secs(15 * 60);

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
    /// 收到的思考链（`reasoning_content` / `reasoning`）分片数。用来排查关思考参数是否生效，不记录任何正文。
    pub reasoning_chunks: usize,
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

#[derive(Debug, Clone)]
struct OriginHealth {
    key: String,
    last_ok_mono: Option<Instant>,
    last_ok_wall: Option<SystemTime>,
    bypass_until: Option<Instant>,
}

static POOL_HEALTH: Mutex<Vec<OriginHealth>> = Mutex::new(Vec::new());

fn pool_origin_key(base_url: &str) -> String {
    if let Ok(url) = reqwest::Url::parse(base_url.trim()) {
        if let Some(host) = url.host_str() {
            return format!(
                "{}://{}:{}",
                url.scheme(),
                host,
                url.port_or_known_default().unwrap_or(0)
            );
        }
    }
    base_url.trim().to_ascii_lowercase()
}

fn should_bypass_pool_at(entry: &OriginHealth, now_mono: Instant, now_wall: SystemTime) -> bool {
    if entry.bypass_until.is_some_and(|until| now_mono < until) {
        return true;
    }
    if let (Some(prev_mono), Some(prev_wall)) = (entry.last_ok_mono, entry.last_ok_wall) {
        let mono_elapsed = now_mono.saturating_duration_since(prev_mono);
        if mono_elapsed < POOL_BYPASS_WINDOW {
            let wall_elapsed = now_wall.duration_since(prev_wall).unwrap_or(mono_elapsed);
            if wall_elapsed > mono_elapsed + SLEEP_WALL_DRIFT {
                return true;
            }
        }
    }
    false
}

fn prune_pool_health(entries: &mut Vec<OriginHealth>, now_mono: Instant) {
    entries.retain(|entry| {
        let recent_ok = entry
            .last_ok_mono
            .is_some_and(|ts| now_mono.saturating_duration_since(ts) < POOL_HEALTH_TTL);
        let active_bypass = entry.bypass_until.is_some_and(|until| now_mono < until);
        recent_ok || active_bypass
    });
}

fn should_bypass_pool(base_url: &str) -> bool {
    let key = pool_origin_key(base_url);
    let now_mono = Instant::now();
    let now_wall = SystemTime::now();
    let mut table = POOL_HEALTH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_pool_health(&mut table, now_mono);
    table
        .iter()
        .find(|entry| entry.key == key)
        .is_some_and(|entry| should_bypass_pool_at(entry, now_mono, now_wall))
}

fn record_pool_probe(base_url: &str, ok: bool, now_mono: Instant, now_wall: SystemTime) {
    let key = pool_origin_key(base_url);
    let mut table = POOL_HEALTH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_pool_health(&mut table, now_mono);
    if let Some(entry) = table.iter_mut().find(|entry| entry.key == key) {
        if ok {
            entry.last_ok_mono = Some(now_mono);
            entry.last_ok_wall = Some(now_wall);
            entry.bypass_until = None;
        } else {
            entry.bypass_until = Some(now_mono + POOL_BYPASS_WINDOW);
        }
    } else {
        table.push(if ok {
            OriginHealth {
                key,
                last_ok_mono: Some(now_mono),
                last_ok_wall: Some(now_wall),
                bypass_until: None,
            }
        } else {
            OriginHealth {
                key,
                last_ok_mono: None,
                last_ok_wall: None,
                bypass_until: Some(now_mono + POOL_BYPASS_WINDOW),
            }
        });
    }
}

fn mark_pool_healthy(base_url: &str) {
    record_pool_probe(base_url, true, Instant::now(), SystemTime::now());
}

fn mark_pool_suspect(base_url: &str) {
    record_pool_probe(base_url, false, Instant::now(), SystemTime::now());
}

/// 预热连接：只做幂等的 `GET /models`（DNS + TCP + TLS 握手），不带 Key、不花 token。
///
/// 不带 Key 时 `/models` 会回 401，但握手已经完成；后续 POST 直接复用连接池，
/// 省掉首次查词 100–300ms 的建连时间。若云端探活失败或超时，则标记该源站连接池可疑，
/// 让即将到来的 POST 直接走无池新连接，避免撞上半死 socket。
pub async fn warmup(client: &reqwest::Client, provider: &Provider) {
    let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
    let is_cloud = !crate::is_loopback(&provider.base_url);
    // 必须把 body 读完：HTTP/1.1 下没读完的响应不能回池，连接会被关掉，
    // 那就只剩 DNS 预热，等于没做。
    let ok = match client
        .get(&url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        Ok(response) => response.text().await.is_ok(),
        Err(_) => false,
    };
    if is_cloud {
        if ok {
            mark_pool_healthy(&provider.base_url);
        } else {
            mark_pool_suspect(&provider.base_url);
        }
    }
}

/// 发一次流式请求，边收边回调。
///
/// `on_delta` 收到的永远是「文本增量」，不是整包；解析失败的分片会被跳过
/// 而不是中断整条流——模型偶尔会吐一个坏 JSON，没必要为此丢掉全部内容。
fn headers_timeout_for(base_url: &str) -> Duration {
    if !crate::is_loopback(base_url) {
        CLOUD_HEADERS_TIMEOUT
    } else {
        HEADERS_TIMEOUT
    }
}

pub async fn stream_chat(
    client: &reqwest::Client,
    provider: &Provider,
    api_key: Option<&str>,
    body: &Value,
    on_delta: &mut (dyn FnMut(&str) + Send),
    cancel: Option<CancelCheck<'_>>,
) -> Result<ChatOutcome, ChatError> {
    let timeout = headers_timeout_for(&provider.base_url);
    stream_chat_with_headers_timeout(
        client,
        provider,
        api_key,
        body,
        on_delta,
        cancel,
        timeout,
    )
    .await
}

async fn send_headers_once(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    body: &Value,
    deadline: Duration,
    headers_timeout: Duration,
) -> Result<reqwest::Response, (ChatError, bool)> {
    let mut request = client.post(url).json(body).timeout(deadline);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    tokio::time::timeout(headers_timeout, request.send())
        .await
        .map_err(|_| {
            (
                ChatError::Timeout(format!("{} 秒内服务端没有开始响应", headers_timeout.as_secs())),
                false,
            )
        })?
        .map_err(|err| {
            let is_connect = err.is_connect();
            if err.is_timeout() {
                (
                    ChatError::Timeout("建立连接或等待服务端响应超时".to_string()),
                    false,
                )
            } else {
                (ChatError::Network(err.to_string()), is_connect)
            }
        })
}

async fn stream_chat_with_headers_timeout(
    client: &reqwest::Client,
    provider: &Provider,
    api_key: Option<&str>,
    body: &Value,
    on_delta: &mut (dyn FnMut(&str) + Send),
    cancel: Option<CancelCheck<'_>>,
    headers_timeout: Duration,
) -> Result<ChatOutcome, ChatError> {
    let is_cloud = !crate::is_loopback(&provider.base_url);
    stream_chat_with_options(
        client,
        provider,
        api_key,
        body,
        on_delta,
        cancel,
        is_cloud,
        headers_timeout,
    )
    .await
}

async fn stream_chat_with_options(
    client: &reqwest::Client,
    provider: &Provider,
    api_key: Option<&str>,
    body: &Value,
    on_delta: &mut (dyn FnMut(&str) + Send),
    cancel: Option<CancelCheck<'_>>,
    is_cloud: bool,
    headers_timeout: Duration,
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

    let started = Instant::now();
    let bypass_pool = is_cloud && should_bypass_pool(&provider.base_url);
    let initial_client = if bypass_pool {
        crate::fresh_client_for(&provider.base_url)
    } else {
        client
    };

    // 非幂等的 POST /chat/completions 只在「确认尚未发出请求体字节」的连接阶段失败（is_connect）时
    // 切换无池新连接重试一次；绝不在等响应头超时时重发，避免慢 prefill / 高峰排队下双倍计费。
    let response = match send_headers_once(
        initial_client,
        &url,
        api_key,
        body,
        deadline,
        headers_timeout,
    )
    .await
    {
        Ok(resp) => {
            if is_cloud && !bypass_pool {
                mark_pool_healthy(&provider.base_url);
            }
            resp
        }
        Err((_first_err, true))
            if is_cloud && !bypass_pool && !cancel.is_some_and(|stopped| stopped()) =>
        {
            mark_pool_suspect(&provider.base_url);
            eprintln!("[sense] 连接建立失败（未发送请求体），换新连接重试一次");
            let fresh = crate::fresh_client_for(&provider.base_url);
            send_headers_once(fresh, &url, api_key, body, deadline, headers_timeout)
                .await
                .map_err(|(err, _)| err)?
        }
        Err((err, _)) => return Err(err),
    };

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
    let mut reasoning_chunks = 0usize;
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

            if value
                .pointer("/choices/0/delta/reasoning_content")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
                || value
                    .pointer("/choices/0/delta/reasoning")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            {
                reasoning_chunks += 1;
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
        reasoning_chunks,
        done,
        cancelled,
    })
}

/// 记住「这家网关不认关思考字段」的 `(base_url|model, 记录时间)`：之后直接不带，省掉每次一个 400 的往返。
/// 带 TTL 过期与自愈清除，避免一次瞬时网关 400 把常驻数天的应用永久锁在开思考模式。
static NO_REASONING: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

fn reasoning_key(provider: &Provider) -> String {
    format!("{}|{}", provider.base_url, provider.model)
}

fn reasoning_rejected_before(provider: &Provider) -> bool {
    let key = reasoning_key(provider);
    let now = Instant::now();
    let mut known = NO_REASONING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    known.retain(|(_, ts)| now.duration_since(*ts) < NO_REASONING_TTL);
    known.iter().any(|(k, _)| k == &key)
}

fn remember_reasoning_rejected(provider: &Provider) {
    let key = reasoning_key(provider);
    let now = Instant::now();
    let mut known = NO_REASONING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    known.retain(|(k, ts)| k != &key && now.duration_since(*ts) < NO_REASONING_TTL);
    known.push((key, now));
}

/// 判断 400 错误体是否像「不认关思考字段」；不能用 `"invalid"` 这类泛词，否则常见 JSON 报错（如 `invalid_request_error`）也会被误记成拒收思考字段。
fn looks_like_reasoning_rejection(err: &ChatError, extra: &Value) -> bool {
    let ChatError::BadRequest(detail) = err else {
        return false;
    };
    let lower = detail.to_ascii_lowercase();
    if let Some(fields) = extra.as_object() {
        if fields.keys().any(|k| lower.contains(&k.to_ascii_lowercase())) {
            return true;
        }
    }
    [
        "reasoning",
        "unknown field",
        "unknown parameter",
        "unknown argument",
        "unrecognized field",
        "unrecognized parameter",
        "unrecognized request argument",
        "unrecognized key",
        "extra_forbidden",
        "extra inputs",
        "extra fields",
        "additional properties",
        "unexpected field",
        "unexpected parameter",
        "unexpected property",
        "unsupported field",
        "unsupported parameter",
        "unpermitted",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
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
            reasoning_chunks: 0,
            done: false,
            cancelled: true,
        }),
        Err(err) if is_bad_request(&err) => {
            eprintln!("[sense] provider 返回 HTTP 400，去掉关思考字段重试一次");
            let plain = without_reasoning(body, extra);
            let retried = stream_chat(client, provider, api_key, &plain, on_delta, cancel).await;
            if retried.is_ok() && looks_like_reasoning_rejection(&err, extra) {
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

    #[tokio::test]
    async fn transient_gateway_400_retries_once_without_poisoning_no_reasoning() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定端口");
        let port = listener.local_addr().expect("取端口").port();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        std::thread::spawn(move || {
            for index in 0..5 {
                let Ok((mut socket, _)) = listener.accept() else { return };
                let request = read_request_text(&mut socket);
                recorded.lock().unwrap().push(request);
                if index == 0 {
                    // 网关偶发 HTML 400（不含任何未知字段字样）
                    let body = "<html><body>400 Bad Request</body></html>";
                    let _ = socket.write_all(
                        format!(
                            "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    );
                } else if index == 3 {
                    // 标准 OpenAI-compatible 泛错误：含 "invalid_request_error"，绝不能误判成「不认 thinking 字段」
                    let body = r#"{"error":{"message":"upstream gateway glitch","type":"invalid_request_error"}}"#;
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
        let base_url = format!("http://127.0.0.1:{port}/v1");

        // 第 1 次：HTML 400 虽靠重试成功，但不毒化 NO_REASONING
        let outcome = ask_adaptive(&base_url).await.expect("去掉字段重试当次仍应成功");
        assert_eq!(outcome.text, "ok");
        // 第 2 次应照旧带回 thinking
        ask_adaptive(&base_url).await.expect("第二次请求仍应成功");
        // 第 3 次：JSON `invalid_request_error` 400 后重试成功，同样不能毒化 NO_REASONING
        let outcome3 = ask_adaptive(&base_url).await.expect("去掉字段重试第三次也应成功");
        assert_eq!(outcome3.text, "ok");
        // 第 4 次（requests[5] 未发生因为第 3 次是 index 3 + index 4）——再验证纯判据与无毒化：
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 5);
        assert!(requests[2].contains("thinking"), "瞬时 HTML 400 不能把 thinking 字段永久关掉");
        assert!(requests[3].contains("thinking"), "invalid_request_error 之前应带 thinking");

        let extra = thinking_off();
        assert!(looks_like_reasoning_rejection(&ChatError::BadRequest(r#"{"error":{"message":"unknown field 'thinking'"}}"#.into()), &extra));
        assert!(looks_like_reasoning_rejection(&ChatError::BadRequest(r#"{"error":{"type":"extra_forbidden"}}"#.into()), &extra));
        assert!(looks_like_reasoning_rejection(&ChatError::BadRequest(r#"{"error":{"message":"unrecognized request argument"}}"#.into()), &extra));
        assert!(looks_like_reasoning_rejection(&ChatError::BadRequest(r#"{"error":{"message":"thinking is not supported for this model","type":"invalid_request_error"}}"#.into()), &extra));
        assert!(!looks_like_reasoning_rejection(&ChatError::BadRequest(r#"{"error":{"message":"upstream gateway glitch","type":"invalid_request_error"}}"#.into()), &extra));
        assert!(!looks_like_reasoning_rejection(&ChatError::BadRequest(r#"{"error":{"message":"context_length_exceeded","type":"invalid_request_error"}}"#.into()), &extra));
        assert!(!looks_like_reasoning_rejection(&ChatError::BadRequest(r#"{"error":{"message":"extra quota needed"}}"#.into()), &extra));
    }

    #[tokio::test]
    async fn openai_json_invalid_request_error_does_not_poison_no_reasoning_on_subsequent_call() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定端口");
        let port = listener.local_addr().expect("取端口").port();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        std::thread::spawn(move || {
            for index in 0..3 {
                let Ok((mut socket, _)) = listener.accept() else { return };
                let request = read_request_text(&mut socket);
                recorded.lock().unwrap().push(request);
                if index == 0 {
                    let body = r#"{"error":{"message":"upstream gateway glitch","type":"invalid_request_error"}}"#;
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
        let base_url = format!("http://127.0.0.1:{port}/v1");

        ask_adaptive(&base_url).await.expect("第一次 400 后重试应成功");
        ask_adaptive(&base_url).await.expect("第二次应成功");
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[2].contains("thinking"), "通用 invalid_request_error 不能毒化 NO_REASONING 缓存");
    }

    #[tokio::test]
    async fn slow_response_headers_do_not_resend_non_idempotent_post() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定端口");
        let port = listener.local_addr().expect("取端口").port();
        let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepted_ref = accepted.clone();
        std::thread::spawn(move || {
            // 连接 1：模拟模型长文 prefill / 排队，延迟 220ms 才开始吐响应头
            if let Ok((mut slow, _)) = listener.accept() {
                accepted_ref.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                read_request(&mut slow);
                std::thread::sleep(Duration::from_millis(220));
                let payload = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
                let _ = slow.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
                        payload.len()
                    )
                    .as_bytes(),
                );
                let _ = slow.flush();
            }
            // 连接 2：在宽限期内同样延迟 220ms 正常完成
            if let Ok((mut slow2, _)) = listener.accept() {
                accepted_ref.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                read_request(&mut slow2);
                std::thread::sleep(Duration::from_millis(220));
                let payload = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
                let _ = slow2.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
                        payload.len()
                    )
                    .as_bytes(),
                );
                let _ = slow2.flush();
            }
        });
        let base_url = format!("http://127.0.0.1:{port}/v1");
        let client = reqwest::Client::builder().no_proxy().build().expect("构建客户端");
        let body = build_body("m", "sys", "user", 0.3, None, 100);

        // 超过 headers_timeout(80ms) 时必须直接报超时，绝不重发第二个非幂等 POST
        let err = stream_chat_with_headers_timeout(
            &client,
            &test_provider(&base_url),
            Some("test-key"),
            &body,
            &mut |_| {},
            None,
            Duration::from_millis(80),
        )
        .await
        .expect_err("等响应头超时后绝不能自动重发 POST");
        assert!(matches!(err, ChatError::Timeout(_)), "应返回 Timeout，实际 {err:?}");
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1, "只能发出 1 次 POST，不能双倍计费");

        // 宽限足够的 headers_timeout(600ms) 内 1 次 POST 完成
        let ok = stream_chat_with_headers_timeout(
            &client,
            &test_provider(&base_url),
            Some("test-key"),
            &body,
            &mut |_| {},
            None,
            Duration::from_millis(600),
        )
        .await
        .expect("慢响应头在单一时限内应一次成功");
        assert_eq!(ok.text, "ok");
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn pool_health_bypasses_pool_after_sleep_clock_jump_or_failed_probe() {
        let mono0 = Instant::now();
        let wall0 = SystemTime::now();
        let healthy = OriginHealth {
            key: "https://api.deepseek.com:443".to_string(),
            last_ok_mono: Some(mono0),
            last_ok_wall: Some(wall0),
            bypass_until: None,
        };
        // 正常经过 2s：不绕过连接池
        assert!(!should_bypass_pool_at(
            &healthy,
            mono0 + Duration::from_secs(2),
            wall0 + Duration::from_secs(2)
        ));
        // 系统休眠：monotonic 只过 1s，wall clock 跳了 30s，且仍在 25s 池窗口内 -> 必须绕过
        assert!(should_bypass_pool_at(
            &healthy,
            mono0 + Duration::from_secs(1),
            wall0 + Duration::from_secs(30)
        ));
        // 若唤醒后连接池窗口已过（26s > 25s），HTTP 客户端自己会淘汰空闲连接 -> 无需绕过
        assert!(!should_bypass_pool_at(
            &healthy,
            mono0 + Duration::from_secs(26),
            wall0 + Duration::from_secs(55)
        ));

        // 探活失败标记 suspect，在窗口内绕过，窗口过后恢复
        let url = "https://sleep-probe.example.com/v1";
        record_pool_probe(url, false, mono0, wall0);
        assert!(should_bypass_pool(url));
        record_pool_probe(url, true, mono0, wall0);
        assert!(!should_bypass_pool(url));
    }

    #[test]
    fn headers_timeout_distinguishes_remote_gateways_from_loopback() {
        assert_eq!(
            headers_timeout_for("https://api.deepseek.com/v1"),
            CLOUD_HEADERS_TIMEOUT
        );
        assert_eq!(
            headers_timeout_for("http://127.0.0.1:11434/v1"),
            HEADERS_TIMEOUT
        );
        assert_eq!(
            headers_timeout_for("http://localhost:11434/v1"),
            HEADERS_TIMEOUT
        );
    }

    #[tokio::test]
    async fn reasoning_chunks_are_counted_and_cache_does_not_oscillate() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定端口");
        let port = listener.local_addr().expect("取端口").port();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept() else { return };
                let _ = read_request_text(&mut socket);
                let payload = concat!(
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think1\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think2\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
                    "data: [DONE]\n\n"
                );
                let _ = socket.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
                        payload.len()
                    )
                    .as_bytes(),
                );
                let _ = socket.flush();
            }
        });
        let base_url = format!("http://127.0.0.1:{port}/v1");
        let provider = test_provider(&base_url);
        remember_reasoning_rejected(&provider);
        assert!(reasoning_rejected_before(&provider));

        let extra = thinking_off();
        let client = reqwest::Client::builder().no_proxy().build().expect("构建客户端");
        let body = build_body("m", "sys", "user", 0.3, Some(&extra), 100);
        let outcome = stream_chat_adaptive(&client, &provider, None, &body, Some(&extra), &mut |_| {}, None)
            .await
            .expect("带 reasoning_content 的流应正常返回");
        assert_eq!(outcome.reasoning_chunks, 2);
        assert_eq!(outcome.text, "ok");
        // 默认会吐思考链但又拒收 thinking 字段的中转站，在 TTL 内不应清缓存，否则下次会隔次再吃 400
        assert!(reasoning_rejected_before(&provider));
    }

    #[tokio::test]
    async fn cloud_slow_response_headers_do_not_resend_non_idempotent_post() {
        use std::io::Write;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定端口");
        let port = listener.local_addr().expect("取端口").port();
        let connections = std::sync::Arc::new(AtomicUsize::new(0));
        let seen_conn = connections.clone();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept() else { return };
                seen_conn.fetch_add(1, Ordering::SeqCst);
                let _ = read_request_text(&mut socket);
                std::thread::sleep(Duration::from_millis(400));
                let payload = "data: {\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\ndata: [DONE]\n\n";
                let _ = socket.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
                        payload.len()
                    )
                    .as_bytes(),
                );
                let _ = socket.flush();
            }
        });

        let provider = test_provider(&format!("http://127.0.0.1:{port}/v1"));
        let client = reqwest::Client::builder().no_proxy().build().expect("构建客户端");
        let body = build_body("m", "sys", "user", 0.3, None, 100);
        let result = stream_chat_with_options(
            &client,
            &provider,
            None,
            &body,
            &mut |_| {},
            None,
            true,
            Duration::from_millis(120),
        )
        .await;
        assert!(matches!(result, Err(ChatError::Timeout(_))), "云端等待响应头超时应直接报错：{result:?}");
        assert_eq!(connections.load(Ordering::SeqCst), 1, "even with is_cloud=true, header timeout must never resend non-idempotent POST");
    }

    #[tokio::test]
    async fn cloud_connect_failure_retries_once_and_marks_pool_suspect() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定端口");
        let port = listener.local_addr().expect("取端口").port();
        drop(listener);

        let provider = test_provider(&format!("http://127.0.0.1:{port}/v1"));
        assert!(!should_bypass_pool(&provider.base_url));
        let client = reqwest::Client::builder().no_proxy().build().expect("构建客户端");
        let body = build_body("m", "sys", "user", 0.3, None, 100);
        let result = stream_chat_with_options(
            &client,
            &provider,
            None,
            &body,
            &mut |_| {},
            None,
            true,
            Duration::from_secs(2),
        )
        .await;
        assert!(matches!(result, Err(ChatError::Network(_))), "连接失败应返回 Network：{result:?}");
        assert!(should_bypass_pool(&provider.base_url), "is_connect 失败重试后应标记连接池 suspect");
    }
}
