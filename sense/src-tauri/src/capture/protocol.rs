//! 截屏翻译与模型之间的协议：怎么分块、消息怎么写、回复怎么按 id 对齐、哪些块算「没译成」。
//!
//! 以前整屏文字做成一个字符串数组、要求等长译回，条数是**唯一**的对齐校验：模型把被侧栏拆开的半句话并回邻条，
//! 条数一少整次就报错（整页约 4 成请求），条数凑对了错位又看不出来。现在每条带一个编号 `{"i":0,"t":"原文"}`，
//! 模型照抄编号译回，应用**按编号**对齐：并掉的那条只是「缺一个编号」——看得见、只重发那几条，不再连累整页。
//!
//! 红线（`MAINTENANCE_RULES.md §2.2`：不许假装成功）落在这里：缺编号、译文为空、同一编号给了两种译文、整段读不出来，
//! 一律是「这一块没译成」，由调用方保留原文并告诉用户；**绝不**按位置猜、也不把它们当成功。
//!
//! 全是纯函数加一个把「问模型」当参数传进来的 `translate_all`，所以失败、重试、分块都能脱离网络单测。
//! 评测与取舍（样本外页面、同一个模型的两批采样加 `deepseek-v4-pro`、按页口径）见 CHANGELOG「截屏翻译：整页『译文条数对不上』改成带编号的协议」。

use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// 每次请求要译的条数上限。整页一次发时（≥41 条）约 4 成请求整次失败；分块后每块短、并行发，整页也更快。
pub(super) const CHUNK_MAX: usize = 30;
/// 各块同时发的上限。整屏文字特别多时（几百块）不能一口气发十几个请求：provider 的限流是按账号来的。
pub(super) const MAX_PARALLEL: usize = 4;
/// 块的两头各多带几条上下文（照常带编号发出，译回的不算），让相邻块的指代、术语接得上。
pub(super) const CONTEXT: usize = 2;
/// 有的块请求本身失败、别的块成功时，那几块整块再问一次之前先等多久：多半是限流（429），紧接着再发多半还是 429。
/// 评测脚本对 429 / 5xx 的退避是 1.5 秒 × 第几次。
pub(super) const RETRY_PAUSE: Duration = Duration::from_millis(1500);

/// 一个块：要译的 `own`，和实际发给模型看的 `shown`（own 两头各加上下文）。下标都是整屏里的块序号。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ChunkPlan {
    pub own: Range<usize>,
    pub shown: Range<usize>,
}

/// 把 `total` 条分成每块至多 `CHUNK_MAX` 条、尽量均分（61 条 → 21 / 20 / 20，而不是 30 / 30 / 1）。
pub(super) fn plan_chunks(total: usize) -> Vec<ChunkPlan> {
    if total == 0 {
        return Vec::new();
    }
    let parts = total.div_ceil(CHUNK_MAX);
    let base = total / parts;
    let extra = total % parts;
    let mut at = 0;
    (0..parts)
        .map(|part| {
            let len = base + usize::from(part < extra);
            let own = at..at + len;
            at += len;
            let shown = own.start.saturating_sub(CONTEXT)..(own.end + CONTEXT).min(total);
            ChunkPlan { own, shown }
        })
        .collect()
}

#[derive(Serialize)]
struct WireItem<'a> {
    i: usize,
    t: &'a str,
}

/// 发给模型的 user 消息：`[{"i":3,"t":"原文"},…]`，紧凑写法，字段顺序固定（i 在前）。
pub(super) fn user_message(texts: &[&str], ids: impl IntoIterator<Item = usize>) -> String {
    let items: Vec<WireItem> = ids.into_iter().map(|i| WireItem { i, t: texts[i] }).collect();
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string())
}

/// 回复里的一个元素。`lenient` = 严格解析失败、靠逐元素扫描捞回来的。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Entry {
    pub i: i64,
    pub t: String,
    pub lenient: bool,
}

/// 解析的结果：捞到的元素，加上扫描时被丢掉的（没有收尾的）元素编号。
#[derive(Debug, Default, PartialEq)]
pub(super) struct Parsed {
    pub entries: Vec<Entry>,
    pub dropped: Vec<i64>,
    /// 宽松扫描时，回复里**最后**一个元素头的编号（不管它收没收尾）；严格解析整段通过时是 `None`。
    /// 回复被 max_tokens 截断时，最后一个元素是在中途被切断的：靠这个编号认出它，别把半截译文当成完整的。
    pub last: Option<i64>,
}

/// 模型偶尔会包一层 ```json，这里统一剥掉。
fn strip_fence(raw: &str) -> &str {
    let mut text = raw.trim();
    if let Some(rest) = text.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest).trim_start();
        text = rest.trim_end_matches('`').trim();
    }
    text
}

/// 解析模型回复。先按严格 JSON 解析整个数组；不行再逐元素扫描——一个元素坏了（模型把第一条写成 `"">…`、
/// 回复被截断、字符串里有没转义的引号）只丢那一个，别的照收，不再整次作废。什么都认不出来才是 Err。
pub(super) fn parse_reply(raw: &str) -> Result<Parsed, String> {
    let text = strip_fence(raw);
    let start = text.find('[').ok_or("译文里没有 JSON 数组")?;
    if let Some(end) = text.rfind(']').filter(|end| *end > start) {
        if let Some(entries) = strict_entries(&text[start..=end]) {
            return Ok(Parsed { entries, dropped: Vec::new(), last: None });
        }
    }
    let parsed = lenient_scan(&text[start..]);
    if parsed.entries.is_empty() {
        return Err("译文里认不出带编号的条目".to_string());
    }
    Ok(parsed)
}

/// 整段是 `[{i:整数, t:字符串}, …]`（字段顺序、多余字段都不管）才算严格通过。
fn strict_entries(text: &str) -> Option<Vec<Entry>> {
    let Value::Array(items) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    items
        .into_iter()
        .map(|item| {
            let object = item.as_object()?;
            let number = object.get("i")?;
            let i = number
                .as_i64()
                .or_else(|| number.as_f64().filter(|value| value.fract() == 0.0 && value.abs() < 9e15).map(|value| value as i64))?;
            let t = object.get("t")?.as_str()?.to_string();
            Some(Entry { i, t, lenient: false })
        })
        .collect()
}

fn skip_ws(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && matches!(bytes[at], b' ' | b'\n' | b'\t' | b'\r') {
        at += 1;
    }
    at
}

/// 跳过空白后匹配字面量，返回匹配之后的位置。
fn eat(bytes: &[u8], at: usize, literal: &str) -> Option<usize> {
    let at = skip_ws(bytes, at);
    bytes[at..].starts_with(literal.as_bytes()).then(|| at + literal.len())
}

/// 在 `at` 处认一个元素头 `{"i": <整数>, "t": "`：认出来给 (编号, t 的字符串内容的起点)。
/// 整数两边各允许一个多余的引号：模型偶尔把编号写成字符串 `"18"`，更常见的是多打一个收尾引号 `{"i":18","t":…}`
/// （评测的 5507 轮回复里 21–22 轮，约 0.4%；一旦出现，这一轮后面的元素往往跟着犯同一个错）。
fn element_header(bytes: &[u8], at: usize) -> Option<(i64, usize)> {
    let mut pos = eat(bytes, at, "{")?;
    pos = eat(bytes, pos, "\"i\"")?;
    pos = eat(bytes, pos, ":")?;
    pos = skip_ws(bytes, pos);
    if bytes.get(pos) == Some(&b'"') {
        pos += 1;
    }
    let digits_start = pos;
    if bytes.get(pos) == Some(&b'-') {
        pos += 1;
    }
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        pos += 1;
    }
    let digits = std::str::from_utf8(&bytes[digits_start..pos]).ok()?;
    let id: i64 = digits.parse().ok()?;
    if bytes.get(pos) == Some(&b'"') {
        pos += 1;
    }
    pos = eat(bytes, pos, ",")?;
    pos = eat(bytes, pos, "\"t\"")?;
    pos = eat(bytes, pos, ":")?;
    pos = eat(bytes, pos, "\"")?;
    Some((id, pos))
}

/// 逐元素扫描：找每个 `{"i":N,"t":"` 的位置，元素的值延伸到下一个元素头（或末尾）之前、
/// 最后一个「引号 + 可选空白 + }」。没有收尾的（被截断）记进 `dropped`。只在严格解析失败时用。
fn lenient_scan(text: &str) -> Parsed {
    let bytes = text.as_bytes();
    let heads: Vec<(usize, i64, usize)> = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'{')
        .filter_map(|(at, _)| element_header(bytes, at).map(|(id, value_start)| (at, id, value_start)))
        .collect();

    let mut parsed = Parsed::default();
    for (index, (_, id, value_start)) in heads.iter().enumerate() {
        let limit = heads.get(index + 1).map_or(bytes.len(), |next| next.0);
        let region = &text[*value_start..limit.max(*value_start)];
        match closing_quote(region.as_bytes()) {
            Some(close) => {
                let (decoded, lenient) = decode_inner(&region[..close]);
                // 认不出的元素头会被上一个元素吞进去：译文里带着 JSON 碎片就是被吞了，宁可当没译成，不能当译文
                if has_element_debris(&decoded) {
                    parsed.dropped.push(*id);
                } else {
                    parsed.entries.push(Entry { i: *id, t: decoded, lenient });
                }
            }
            None => parsed.dropped.push(*id),
        }
    }
    parsed.last = heads.last().map(|(_, id, _)| *id);
    parsed
}

/// 译文里出现元素边界 `"` `}` `,` `{` 或元素的键 `"i":` / `"t":`：多半是别的元素被吞进来了。只在宽松路径用。
fn has_element_debris(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().filter(|(_, byte)| **byte == b'"').any(|(k, _)| {
        // 键：" + i 或 t + " + 冒号
        let p = skip_ws(bytes, k + 1);
        let key = matches!(bytes.get(p), Some(b'i' | b't'))
            && bytes.get(p + 1) == Some(&b'"')
            && bytes.get(skip_ws(bytes, p + 2)) == Some(&b':');
        // 边界："} , {
        let boundary = bytes.get(p) == Some(&b'}') && {
            let q = skip_ws(bytes, p + 1);
            bytes.get(q) == Some(&b',') && bytes.get(skip_ws(bytes, q + 1)) == Some(&b'{')
        };
        key || boundary
    })
}

/// 区域里最后一个「引号 + 可选空白 + }」的引号位置。
fn closing_quote(region: &[u8]) -> Option<usize> {
    (0..region.len()).rev().filter(|k| region[*k] == b'}').find_map(|k| {
        let mut q = k;
        while q > 0 && matches!(region[q - 1], b' ' | b'\n' | b'\t' | b'\r') {
            q -= 1;
        }
        // 用 then 而不是 then_some：后者的参数立即求值，q 为 0 时 q - 1 会下溢
        (q > 0 && region[q - 1] == b'"').then(|| q - 1)
    })
}

/// 一段 JSON 字符串内容（不含两头的引号）→ 文字。严格解得开就用严格的；否则宽松：
/// 裸引号当字面引号，未知转义原样留下。第一个元素被写成 `"">…`（开头多一个裸引号）时去掉那一个。
fn decode_inner(inner: &str) -> (String, bool) {
    if let Ok(text) = serde_json::from_str::<String>(&format!("\"{inner}\"")) {
        return (text, false);
    }
    let chars: Vec<char> = inner.chars().collect();
    let mut out = String::new();
    let mut k = 0;
    while k < chars.len() {
        let ch = chars[k];
        if ch != '\\' {
            out.push(ch);
            k += 1;
            continue;
        }
        let Some(&next) = chars.get(k + 1) else {
            out.push('\\');
            break;
        };
        let simple = match next {
            '"' => Some('"'),
            '\\' => Some('\\'),
            '/' => Some('/'),
            'b' => Some('\u{8}'),
            'f' => Some('\u{c}'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            _ => None,
        };
        if let Some(simple) = simple {
            out.push(simple);
            k += 2;
        } else if next == 'u' {
            match hex4(&chars, k + 2) {
                Some(unit) => {
                    k += 6;
                    // 代理对：高位后面紧跟 \uDC00–\uDFFF 才能拼成一个字
                    if (0xd800..0xdc00).contains(&unit) && chars.get(k) == Some(&'\\') && chars.get(k + 1) == Some(&'u') {
                        if let Some(low) = hex4(&chars, k + 2).filter(|low| (0xdc00..0xe000).contains(low)) {
                            let code = 0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00);
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            k += 6;
                            continue;
                        }
                    }
                    out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
                }
                None => {
                    out.push_str("\\u");
                    k += 2;
                }
            }
        } else {
            out.push('\\');
            out.push(next);
            k += 2;
        }
    }
    if inner.starts_with('"') && out.starts_with('"') {
        out.remove(0);
    }
    (out, true)
}

fn hex4(chars: &[char], at: usize) -> Option<u32> {
    let digits: String = chars.get(at..at + 4)?.iter().collect();
    (digits.len() == 4 && digits.chars().all(|ch| ch.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(&digits, 16).ok())
        .flatten()
}

/// 一个块这一轮的结局。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Slot {
    Ok(String),
    /// 回复里没有这个编号
    Missing,
    /// 译文是空的
    Empty,
    /// 同一个编号给了两种译文，谁也不信
    Conflict,
}

/// 按编号对齐。`want` 是这一轮要的编号；不在里面的（上下文、编造的）只计数。
/// 内容相同的重复无害；内容不同的重复谁也不信。返回 (与 `want` 同序的各块结局, 多余的编号数)。
pub(super) fn align(entries: &[Entry], want: &[usize]) -> (Vec<Slot>, usize) {
    let mut slots: HashMap<usize, Slot> = HashMap::new();
    let mut stray = 0;
    for entry in entries {
        let Some(id) = usize::try_from(entry.i).ok().filter(|id| want.contains(id)) else {
            stray += 1;
            continue;
        };
        let text = entry.t.trim();
        match slots.get(&id) {
            None => {
                slots.insert(id, if text.is_empty() { Slot::Empty } else { Slot::Ok(text.to_string()) });
            }
            Some(Slot::Ok(previous)) if previous == text => {}
            // 两次都是空的：还是「译文是空的」，不是「给了两种译文」
            Some(Slot::Empty) if text.is_empty() => {}
            Some(_) => {
                slots.insert(id, Slot::Conflict);
            }
        }
    }
    (want.iter().map(|id| slots.remove(id).unwrap_or(Slot::Missing)).collect(), stray)
}

/// 对齐粗检（长度剖面，Gale–Church 式）。按编号对齐只保证「编号对得上」，不保证「内容对得上」：模型偶尔编号照抄、
/// 译文却整体错了一位。对齐的译文长度与原文成比例，错一位以后每条译文的长度
/// 对应的是邻条原文——在滑动窗口里比较「译文 k 配原文 k」「配原文 k±1」三种配法的失配，邻位明显更好、本位本身又不好，才算可疑。
/// 阈值取自选择集；检出率、误报率、它抓不到什么（PDF 里「顺着句意重新分配」的漂移、≤6 块的输入一个窗口都没有）见 CHANGELOG
/// 「截屏翻译：整页『译文条数对不上』改成带编号的协议」的表 7 与已知限制。下面这些常量改动前要重新量，`tests` 里把它们钉住了。
///
/// 权重：汉字 1，拉丁字母与数字 0.5，其余（空格、标点）0.3。
fn weight(text: &str) -> f64 {
    text.chars()
        .map(|ch| match ch {
            '\u{4e00}'..='\u{9fff}' => 1.0,
            ch if ch.is_ascii_alphanumeric() => 0.5,
            _ => 0.3,
        })
        .sum()
}

/// 译成什么方向。对齐粗检的长度比只量过中↔英；别的目标语言（日、韩、法……）没量过，不检，宁可漏检也不误报。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Direction {
    ToEnglish,
    ToChinese,
    Other,
}

impl From<bool> for Direction {
    fn from(to_english: bool) -> Self {
        if to_english { Direction::ToEnglish } else { Direction::ToChinese }
    }
}

/// 译文权重 / 原文权重 的典型值（所有已译出的块的中位数）：译成英文 1.57，译成中文 0.65。
const RATIO_TO_ENGLISH: f64 = 1.57;
const RATIO_TO_CHINESE: f64 = 0.65;
/// 窗口大小；本位配法的平均失配至少这么大；邻位配法的总失配至少要比本位低这么多（单位：自然对数）。
const CHECK_WINDOW: usize = 5;
const CHECK_TAU: f64 = 0.55;
const CHECK_MARGIN: f64 = 1.2;
/// 对数比的平滑项（给两边的权重各加这么多）：很短的条目（菜单项、单个词）长度比本来就不稳，不能被放大成「失配」。
const CHECK_SMOOTH: f64 = 2.0;

/// 可疑窗口的起点。`translations` 与 `texts` 等长，没译成的是 `None`；窗口里有 `None`、或贴着边界没有邻位就跳过。
pub(super) fn suspicious_windows(texts: &[&str], translations: &[Option<&str>], direction: impl Into<Direction>) -> Vec<usize> {
    let ratio = match direction.into() {
        Direction::ToEnglish => RATIO_TO_ENGLISH,
        Direction::ToChinese => RATIO_TO_CHINESE,
        Direction::Other => return Vec::new(),
    };
    let cost = |k: usize, j: Option<usize>| -> Option<f64> {
        let translated = translations.get(j?)?.as_ref()?;
        Some(((weight(translated) + CHECK_SMOOTH) / (weight(texts[k]) * ratio + CHECK_SMOOTH)).ln().abs())
    };
    let mut hits = Vec::new();
    for start in 0..(texts.len() + 1).saturating_sub(CHECK_WINDOW) {
        let (mut aligned, mut next, mut previous) = (0.0, 0.0, 0.0);
        let mut complete = true;
        for k in start..start + CHECK_WINDOW {
            match (cost(k, Some(k)), cost(k, Some(k + 1)), cost(k, k.checked_sub(1))) {
                (Some(a), Some(n), Some(p)) => {
                    aligned += a;
                    next += n;
                    previous += p;
                }
                _ => {
                    complete = false;
                    break;
                }
            }
        }
        if complete && aligned / CHECK_WINDOW as f64 >= CHECK_TAU && aligned - next.min(previous) >= CHECK_MARGIN {
            hits.push(start);
        }
    }
    hits
}

/// 一块没译成的原因：给用户看的话。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Why {
    /// 请求本身失败（没 Key、网络、限流……），带着应用已有的人话
    Request(String),
    /// 模型回了，但整段读不出来
    Unreadable(String),
    /// 回复被截断了，这一条没轮到
    Truncated,
    Missing,
    Empty,
    Conflict,
    /// 对齐粗检：这一块的译文长度像是错位了一格
    Misaligned,
}

impl Why {
    pub(super) fn describe(&self) -> String {
        match self {
            Why::Request(message) => message.clone(),
            Why::Unreadable(detail) => format!("模型的回复读不出来（{detail}）"),
            Why::Truncated => "模型的回复太长被截断了".to_string(),
            Why::Missing => "模型没有给出这几条的译文".to_string(),
            Why::Empty => "模型给出的译文是空的".to_string(),
            Why::Conflict => "模型对同一条给出了两种译文".to_string(),
            Why::Misaligned => "译文的长度和原文对不上，像是错了位".to_string(),
        }
    }

    /// 总失败时只能说一句：请求失败（Key、网络）最能指导用户去做什么，排第一。
    fn rank(&self) -> u8 {
        match self {
            Why::Request(_) => 0,
            Why::Unreadable(_) => 1,
            Why::Truncated => 2,
            Why::Missing => 3,
            Why::Empty => 4,
            Why::Conflict => 5,
            Why::Misaligned => 6,
        }
    }
}

/// 模型的一次回复。`truncated` = 撞到了 max_tokens，内容是被截断的。
#[derive(Debug, Clone, Default)]
pub(super) struct Reply {
    pub text: String,
    pub truncated: bool,
}

/// 整屏翻译的结果。`translations` 与块一一对应：`Ok` 是译文，`Err` 是这一块没译成的原因。
#[derive(Debug)]
pub(super) struct Outcome {
    pub translations: Vec<Result<String, Why>>,
    /// 一共向模型发了几次请求、其中几次是「只重发没译成的那几条」
    pub requests: usize,
    pub retries: usize,
}

impl Outcome {
    pub(super) fn failed(&self) -> usize {
        self.translations.iter().filter(|item| item.is_err()).count()
    }

    /// 没译成的块里最该告诉用户的那条原因（同一级里取出现最多的）。
    pub(super) fn main_reason(&self) -> Option<&Why> {
        let mut best: Option<(&Why, usize)> = None;
        for why in self.translations.iter().filter_map(|item| item.as_ref().err()) {
            let count = self.translations.iter().filter(|item| item.as_ref().err() == Some(why)).count();
            let better = match best {
                None => true,
                Some((current, current_count)) => (why.rank(), usize::MAX - count) < (current.rank(), usize::MAX - current_count),
            };
            if better {
                best = Some((why, count));
            }
        }
        best.map(|(why, _)| why)
    }
}

/// 问一次模型：参数是 user 消息，返回模型的回复或一句人话的错误。
/// 调用方把 system 提示词、provider、Key 都包在闭包里。
pub(super) trait Ask: Fn(String) -> Self::Fut {
    type Fut: Future<Output = Result<Reply, String>>;
}

impl<F, Fut> Ask for F
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Reply, String>>,
{
    type Fut = Fut;
}

/// 一个块跑完的结果。
struct ChunkRun {
    /// 这一块里每一条的结局，与 `plan.own` 同序
    items: Vec<Result<String, Why>>,
    /// 这一块自己向模型发了几次请求、其中几次是补问
    requests: usize,
    retries: usize,
    /// 第一次请求本身就失败了（没 Key、断网、限流……）：别的块成功时，只有这样的块值得整块再问一次
    first_request_failed: bool,
}

/// 一个块：问一次；之后至多再问一次（所以一块自己最多两次请求）——
/// - 译文里有可疑窗口（像是错了位）：这一块的回复整个不可信，整块重发（同一条消息），第二次的回复整个换上来；
/// - 否则，没译成的编号只重发那几条（不带上下文）。请求本身失败（没 Key、断网、限流）不重发：重发多半还是同样的错，只会让用户多等一轮。
/// 最后再查一遍：仍然可疑，这一块的译文**整块**降级成「没译成」——可疑窗口只覆盖错位的开头，错位多半一直延续到块尾
/// （评测里见过 15 块的错位只有 5–6 块落在窗口里），只降级窗口会让后面错位的译文照样当成功显示；宁可保留原文告诉用户。
///
/// 第一次检查会跳过含「没译成」的窗口（缺一条，前后 7 个窗口就没法查）。所以错位只在补缺**之后**才查得出来时，
/// 这一块**直接整块降级、不再整块重发**：补缺那一步已经是第二次请求，再整块问一遍就是第三次。
/// 评测记录里这条路径出现过 3 次，都不是最终方案的记录。
async fn run_chunk<A: Ask>(texts: &[&str], plan: &ChunkPlan, to_english: Direction, ask: &A) -> ChunkRun {
    let want: Vec<usize> = plan.own.clone().collect();
    let shown: Vec<usize> = plan.shown.clone().collect();
    let (mut requests, mut retries) = (0, 0);
    let mut result = ask_round(texts, shown.clone(), &want, ask, &mut requests).await;
    let first_request_failed = result.iter().all(|item| matches!(item, Err(Why::Request(_))));

    if !misaligned(texts, plan.own.start, &result, to_english).is_empty() {
        retries += 1;
        let again = ask_round(texts, shown, &want, ask, &mut requests).await;
        // 第二次整个请求都失败了（断网）就退回第一次的回复，由下面的降级兜底
        if !again.iter().all(|item| matches!(item, Err(Why::Request(_)))) {
            result = again;
        }
    } else {
        let failed: Vec<usize> = want
            .iter()
            .zip(&result)
            .filter(|(_, item)| item.is_err())
            .filter(|(_, item)| !matches!(item, Err(Why::Request(_))))
            .map(|(id, _)| *id)
            .collect();
        if !failed.is_empty() {
            retries += 1;
            let again = ask_round(texts, failed.clone(), &failed, ask, &mut requests).await;
            for (id, item) in failed.iter().zip(again) {
                result[id - plan.own.start] = item;
            }
        }
    }

    if !misaligned(texts, plan.own.start, &result, to_english).is_empty() {
        for item in result.iter_mut().filter(|item| item.is_ok()) {
            *item = Err(Why::Misaligned);
        }
    }
    ChunkRun { items: result, requests, retries, first_request_failed }
}

/// 这一块里有没有可疑窗口（译文按块首起算的下标）。
fn misaligned(texts: &[&str], first: usize, result: &[Result<String, Why>], to_english: Direction) -> Vec<usize> {
    let own: Vec<&str> = (0..result.len()).map(|k| texts[first + k]).collect();
    let translations: Vec<Option<&str>> = result.iter().map(|item| item.as_ref().ok().map(String::as_str)).collect();
    suspicious_windows(&own, &translations, to_english)
}

/// 发一次请求（`shown` 里的编号发出去）并按 `want` 对齐。
async fn ask_round<A: Ask>(texts: &[&str], shown: Vec<usize>, want: &[usize], ask: &A, requests: &mut usize) -> Vec<Result<String, Why>> {
    *requests += 1;
    let reply = match ask(user_message(texts, shown)).await {
        Ok(reply) => reply,
        Err(message) => return want.iter().map(|_| Err(Why::Request(message.clone()))).collect(),
    };
    let mut parsed = match parse_reply(&reply.text) {
        Ok(parsed) => parsed,
        Err(detail) => {
            let why = if reply.truncated { Why::Truncated } else { Why::Unreadable(detail) };
            return want.iter().map(|_| Err(why.clone())).collect();
        }
    };
    // 回复被截断时，最后一个元素是在中途被切断的：宽松扫描偶尔会在半截译文里碰到 `"}`（译文里带着 JSON 字样时）而把它当成完整的。
    // 宁可把那一条当没译成、补问一次，也不显示半句话
    if reply.truncated && parsed.last.is_some() && parsed.entries.last().map(|entry| entry.i) == parsed.last {
        parsed.entries.pop();
    }
    let (slots, _) = align(&parsed.entries, want);
    slots
        .into_iter()
        .map(|slot| match slot {
            Slot::Ok(text) => Ok(text),
            Slot::Missing if reply.truncated => Err(Why::Truncated),
            Slot::Missing => Err(Why::Missing),
            Slot::Empty => Err(Why::Empty),
            Slot::Conflict => Err(Why::Conflict),
        })
        .collect()
}

/// 把整屏的块分块、问模型、按编号对齐、对没译成的再问一次，返回每一块的结局。
/// `parallel` = 各块同时发（云端 provider，同时至多 `MAX_PARALLEL` 个）；本地模型一次只能处理一个，同时发只会排队超时，就一块一块来。
/// `to_english` = 译成英文（否则译成中文），对齐粗检按它取长度比。
///
/// 同时发出去的请求里，有的块**第一次请求本身就失败**、别的块成功了：多半是限流（429）或一次偶发的网络错误，
/// 等 `RETRY_PAUSE` 之后把那几块整块再问一次（再问的这一次自己也还可以补缺 / 整块重发一次，所以一块最多 3 次请求）。
/// 一个块都没成功时不再问：Key 不对、断网，再问一遍只会让用户多等一轮才看到同样的错。
/// 第一次请求成功、补缺那一步才失败的块不再整块问：回复本身就有问题，再问多半还要补缺，请求数会涨到 4 次。
/// 本地模型（`parallel` 为假）一块一块来，没有这一步。
pub(super) async fn translate_all<A: Ask>(texts: &[&str], parallel: bool, to_english: impl Into<Direction>, ask: A) -> Outcome {
    translate_all_with(texts, parallel, to_english, RETRY_PAUSE, ask).await
}

/// `translate_all`，等多久再问可调（测试里是 0，不用起计时器）。
async fn translate_all_with<A: Ask>(texts: &[&str], parallel: bool, to_english: impl Into<Direction>, pause: Duration, ask: A) -> Outcome {
    let to_english = to_english.into();
    use futures_util::StreamExt;

    let plans = plan_chunks(texts.len());
    let mut chunks: Vec<ChunkRun> = Vec::with_capacity(plans.len());
    if parallel {
        // 先收成 Vec 再交给流：直接把带借用的闭包塞进流里，Send 的推导会报「FnOnce is not general enough」
        let runs: Vec<_> = plans
            .iter()
            .enumerate()
            .map(|(index, plan)| {
                let ask = &ask;
                async move { (index, run_chunk(texts, plan, to_english, ask).await) }
            })
            .collect();
        // 不用 buffered：它按顺序交出结果，前面一块慢，后面已经完成的也占着名额，新的块发不出去
        let mut done: Vec<(usize, ChunkRun)> = futures_util::stream::iter(runs).buffer_unordered(MAX_PARALLEL).collect().await;
        done.sort_by_key(|(index, _)| *index);
        chunks.extend(done.into_iter().map(|(_, run)| run));

        let someone_succeeded = chunks.iter().any(|chunk| chunk.items.iter().any(Result::is_ok));
        if someone_succeeded {
            let mut waited = false;
            for (plan, chunk) in plans.iter().zip(chunks.iter_mut()) {
                if !chunk.first_request_failed {
                    continue;
                }
                // 被限流的块紧接着再发多半还是被限流：先等一等（几块一起只等一次）
                if !waited && !pause.is_zero() {
                    tokio::time::sleep(pause).await;
                }
                waited = true;
                let again = run_chunk(texts, plan, to_english, &ask).await;
                chunk.requests += again.requests;
                chunk.retries += again.retries + 1;
                chunk.items = again.items;
            }
        }
    } else {
        for plan in &plans {
            chunks.push(run_chunk(texts, plan, to_english, &ask).await);
        }
    }

    let requests = chunks.iter().map(|chunk| chunk.requests).sum();
    let retries = chunks.iter().map(|chunk| chunk.retries).sum();
    let translations = chunks.into_iter().flat_map(|chunk| chunk.items).collect();
    Outcome { translations, requests, retries }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn entry(i: i64, t: &str) -> Entry {
        Entry { i, t: t.to_string(), lenient: false }
    }

    fn ids(parsed: &Parsed) -> Vec<(i64, String)> {
        parsed.entries.iter().map(|item| (item.i, item.t.clone())).collect()
    }

    // ---- 分块 ----

    #[test]
    fn chunks_are_balanced_and_never_leave_a_tiny_tail() {
        let own = |total: usize| plan_chunks(total).into_iter().map(|plan| plan.own).collect::<Vec<_>>();
        assert_eq!(own(0), Vec::<Range<usize>>::new());
        assert_eq!(own(1), vec![0..1]);
        assert_eq!(own(30), vec![0..30]);
        assert_eq!(own(31), vec![0..16, 16..31]);
        assert_eq!(own(61), vec![0..21, 21..41, 41..61]);
        assert_eq!(own(96), vec![0..24, 24..48, 48..72, 72..96]);
    }

    #[test]
    fn every_block_is_in_exactly_one_chunk_and_context_stays_inside_the_screen() {
        for total in [1, 2, 29, 30, 31, 59, 60, 61, 90, 91, 130] {
            let plans = plan_chunks(total);
            let mut covered = Vec::new();
            for plan in &plans {
                assert!(plan.own.len() <= CHUNK_MAX, "total={total}");
                assert!(plan.shown.start <= plan.own.start && plan.own.end <= plan.shown.end);
                assert!(plan.own.start - plan.shown.start <= CONTEXT && plan.shown.end - plan.own.end <= CONTEXT);
                assert!(plan.shown.end <= total);
                covered.extend(plan.own.clone());
            }
            assert_eq!(covered, (0..total).collect::<Vec<_>>(), "total={total}");
        }
        let plans = plan_chunks(61);
        assert_eq!(plans[0].shown, 0..23, "第一块前面没有上下文");
        assert_eq!(plans[1].shown, 19..43);
        assert_eq!(plans[2].shown, 39..61, "最后一块后面没有上下文");
    }

    // ---- 消息 ----

    #[test]
    fn the_user_message_is_compact_json_with_global_ids_and_escaped_text() {
        let texts = ["a", "say \"hi\"\n", "中文 {\"i\":1}"];
        assert_eq!(
            user_message(&texts, [1, 2]),
            r#"[{"i":1,"t":"say \"hi\"\n"},{"i":2,"t":"中文 {\"i\":1}"}]"#
        );
        assert_eq!(user_message(&texts, std::iter::empty()), "[]");
        // 发出去的东西自己能读回来
        let back = parse_reply(&user_message(&texts, 0..3)).unwrap();
        assert_eq!(ids(&back), vec![(0, "a".into()), (1, "say \"hi\"\n".into()), (2, "中文 {\"i\":1}".into())]);
    }

    // ---- 解析 ----

    #[test]
    fn strict_reply_is_read_as_is_whatever_the_field_order_and_fences() {
        let parsed = parse_reply("```json\n[{\"t\":\"甲\",\"i\":0,\"note\":\"x\"},{\"i\":1.0,\"t\":\"乙\"}]\n```").unwrap();
        assert_eq!(ids(&parsed), vec![(0, "甲".into()), (1, "乙".into())]);
        assert!(parsed.entries.iter().all(|item| !item.lenient));
        assert!(parsed.dropped.is_empty());
        assert_eq!(parse_reply("[]").unwrap().entries.len(), 0, "空数组是合法的回复：全部缺失，由对齐去报");
    }

    #[test]
    fn a_doubled_opening_quote_on_the_first_element_is_repaired_without_failing_the_whole_reply() {
        let parsed = parse_reply(r#"[{"i":0,"t":"">Classes"},{"i":1,"t":"Next"}]"#).unwrap();
        assert_eq!(ids(&parsed), vec![(0, ">Classes".into()), (1, "Next".into())]);
        assert!(parsed.entries[0].lenient && !parsed.entries[1].lenient);
    }

    #[test]
    fn a_stray_quote_after_the_id_is_tolerated_and_no_element_swallows_the_next() {
        // 真实回复（deepseek-chat，en-tauri-start）里的写法：从第 18 条起编号后面多一个引号
        let raw = r#"[{"i":16,"t":"项目结构"},{"i":17,"t":"灵活的架构"},{"i":18","t":"。开发者可以集成任何可编译为 HTML、"},{"i":19","t":"前端配置"}]"#;
        let parsed = parse_reply(raw).unwrap();
        assert_eq!(
            ids(&parsed),
            vec![(16, "项目结构".into()), (17, "灵活的架构".into()), (18, "。开发者可以集成任何可编译为 HTML、".into()), (19, "前端配置".into())]
        );
        assert!(parsed.dropped.is_empty());
        // 编号两边的引号各自可有可无
        let parsed = parse_reply(r#"[{"i":"7","t":"甲"},{"i":8","t":"乙"},{"i":"9,"t":"丙"}"#).unwrap();
        assert_eq!(ids(&parsed).iter().map(|item| item.0).collect::<Vec<_>>(), vec![7, 8, 9]);
    }

    #[test]
    fn an_element_whose_header_cannot_be_recognised_is_never_swallowed_into_the_previous_translation() {
        // 键没加引号：认不出这个元素头，上一条会把它吞进去——吞了碎片的那条必须当没译成，而不是带着 JSON 碎片当译文
        let parsed = parse_reply(r#"[{"i":0,"t":"甲"},{"i":1,"t":"乙"},{i:2,"t":"丙"},{"i":3,"t":"丁"}]"#).unwrap();
        assert_eq!(ids(&parsed), vec![(0, "甲".into()), (3, "丁".into())]);
        assert_eq!(parsed.dropped, vec![1]);
        let (slots, _) = align(&parsed.entries, &[0, 1, 2, 3]);
        assert_eq!(slots, vec![Slot::Ok("甲".into()), Slot::Missing, Slot::Missing, Slot::Ok("丁".into())]);
        // 严格解析通过的译文里本来就有这些字符，不受影响
        let parsed = parse_reply(r#"[{"i":0,"t":"键 \"i\": 1"},{"i":1,"t":"a\"},{\"b"}]"#).unwrap();
        assert_eq!(parsed.entries[0].t, "键 \"i\": 1");
        assert!(!parsed.entries[0].lenient);
    }

    #[test]
    fn a_reply_cut_off_right_where_a_value_starts_is_unreadable_not_a_panic() {
        // 元素头之后紧跟 `}`、前面没有引号：区域的第 0 个字节就是 `}`。对拍时随机变异出来的，曾经下溢
        for raw in [r#"[{"i":0,"t":"}"#, r#"[{"i":0,"t":" }"#, r#"[{"i":0,"t":""#, r#"[{"i":0,"t":"#, r#"[{"i":"#, r#"[{"#, "["] {
            assert!(parse_reply(raw).is_err(), "{raw}");
        }
        let parsed = parse_reply(r#"[{"i":0,"t":"甲"},{"i":1,"t":"}"#).unwrap();
        assert_eq!(ids(&parsed), vec![(0, "甲".into())]);
        assert_eq!(parsed.dropped, vec![1]);
    }

    #[test]
    fn an_escaped_opening_quote_is_a_real_quote_and_stays() {
        let parsed = parse_reply(r#"[{"i":0,"t":"\"hi\" said"},{"i":1,"t":"ok" x"}]"#).unwrap();
        assert_eq!(ids(&parsed), vec![(0, "\"hi\" said".into()), (1, "ok\" x".into())]);
    }

    #[test]
    fn a_truncated_reply_keeps_the_complete_elements_and_names_the_cut_one() {
        let parsed = parse_reply(r#"[{"i":0,"t":"甲"},{"i":1,"t":"乙"},{"i":2,"t":"丙丙"#).unwrap();
        assert_eq!(ids(&parsed), vec![(0, "甲".into()), (1, "乙".into())]);
        assert_eq!(parsed.dropped, vec![2]);
    }

    #[test]
    fn quoted_ids_tabs_and_newlines_are_accepted_by_the_lenient_scan() {
        let parsed = parse_reply("[{\"i\":\"3\",\"t\":\"a\tb\"},{\"i\":4,\"t\":\"c\nd\"}").unwrap();
        assert_eq!(ids(&parsed), vec![(3, "a\tb".into()), (4, "c\nd".into())]);
    }

    #[test]
    fn json_looking_text_inside_a_translation_does_not_split_the_element() {
        let parsed = parse_reply(r#"[{"i":0,"t":"用 {"a":1} 表示"},{"i":1,"t":"好"}]"#).unwrap();
        assert_eq!(ids(&parsed), vec![(0, "用 {\"a\":1} 表示".into()), (1, "好".into())]);
    }

    #[test]
    fn lenient_decoding_handles_escapes_unicode_and_surrogate_pairs() {
        let parsed = parse_reply(r#"[{"i":0,"t":"a中😀\q"},{"i":1,"t":"x" y"}]"#).unwrap();
        assert_eq!(parsed.entries[0].t, "a中😀\\q");
        assert_eq!(parsed.entries[1].t, "x\" y");
    }

    #[test]
    fn a_reply_that_is_not_in_the_id_format_is_unreadable_never_guessed_by_position() {
        assert!(parse_reply("对不起，我不能翻译").is_err());
        assert!(parse_reply("没有数组").unwrap_err().contains("没有 JSON 数组"));
        assert!(parse_reply(r#"["甲","乙"]"#).is_err(), "回了字符串数组（没按编号）：不能按位置对");
        assert!(parse_reply("[{\"i\":0}]").is_err(), "没有 t");
    }

    // ---- 对齐 ----

    #[test]
    fn alignment_reports_missing_empty_conflicting_and_stray_ids_instead_of_succeeding() {
        let entries = [
            entry(0, "甲"),
            entry(1, "  "),
            entry(2, "丙"),
            entry(2, " 丙 "), // 内容相同的重复：无害
            entry(3, "丁"),
            entry(3, "别的"), // 内容不同的重复：谁也不信
            entry(9, "多余"),
            entry(-1, "负数"),
        ];
        let (slots, stray) = align(&entries, &[0, 1, 2, 3, 4, 5]);
        assert_eq!(stray, 2);
        assert_eq!(slots[0], Slot::Ok("甲".into()));
        assert_eq!(slots[1], Slot::Empty);
        assert_eq!(slots[2], Slot::Ok("丙".into()));
        assert_eq!(slots[3], Slot::Conflict);
        assert_eq!(slots[4], Slot::Missing);
        assert_eq!(slots[5], Slot::Missing);
    }

    #[test]
    fn alignment_goes_by_id_not_by_position() {
        let (slots, _) = align(&[entry(11, "乙"), entry(10, "甲")], &[10, 11]);
        assert_eq!(slots, vec![Slot::Ok("甲".into()), Slot::Ok("乙".into())]);
    }

    // ---- 整屏：分块、重试、部分失败 ----

    /// 假模型：按 user 里的编号逐条回 `T<i>`；`drop` 决定这一轮丢掉哪些编号（参数是 (这一轮第几次调用, 编号)）。
    fn model<'a>(calls: &'a Mutex<Vec<String>>, drop: impl Fn(usize, i64) -> bool + 'a) -> impl Fn(String) -> std::future::Ready<Result<Reply, String>> + 'a {
        move |user: String| {
            let call = {
                let mut seen = calls.lock().unwrap();
                seen.push(user.clone());
                seen.len()
            };
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let kept: Vec<Value> = items
                .iter()
                .filter(|item| !drop(call, item["i"].as_i64().unwrap()))
                .map(|item| serde_json::json!({"i": item["i"], "t": format!("T{}", item["i"])}))
                .collect();
            ready(Ok(Reply { text: Value::Array(kept).to_string(), truncated: false }))
        }
    }

    fn texts(n: usize) -> Vec<String> {
        (0..n).map(|k| format!("source {k}")).collect()
    }

    /// 不等的版本：测试里的运行时没开计时器，「再问之前先等一等」单独在 `the_re_ask_waits_before_it_goes_out` 里测。
    fn run(texts: &[String], parallel: bool, ask: impl Ask) -> Outcome {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(translate_all_with(&refs, parallel, false, Duration::ZERO, ask))
    }

    #[test]
    fn a_whole_screen_is_translated_chunk_by_chunk_and_context_replies_are_ignored() {
        let calls = Mutex::new(Vec::new());
        let source = texts(70);
        let outcome = run(&source, true, model(&calls, |_, _| false));
        assert_eq!(outcome.translations.len(), 70);
        for (k, item) in outcome.translations.iter().enumerate() {
            assert_eq!(item.as_ref().ok(), Some(&format!("T{k}")), "第 {k} 块");
        }
        assert_eq!((outcome.requests, outcome.retries, outcome.failed()), (3, 0, 0));
        let calls = calls.lock().unwrap();
        assert!(calls.iter().any(|user| user.contains(r#""i":0,"#)) && calls.iter().any(|user| user.contains(r#""i":69,"#)));
        // 70 条 → 24 / 23 / 23；第二块自己是 24..47，前后各带 2 条上下文：编号 22..=48
        let second: Vec<Value> = serde_json::from_str(calls.iter().find(|user| user.contains(r#""i":24,"#) && !user.contains(r#""i":0,"#)).unwrap()).unwrap();
        assert_eq!((second.first().unwrap()["i"].as_i64(), second.last().unwrap()["i"].as_i64()), (Some(22), Some(48)));
    }

    #[test]
    fn a_dropped_id_is_asked_again_alone_and_recovered() {
        let calls = Mutex::new(Vec::new());
        let source = texts(12);
        // 第 1 次调用丢掉 7；重发（第 2 次调用）正常回
        let outcome = run(&source, true, model(&calls, |call, id| call == 1 && id == 7));
        assert_eq!(outcome.failed(), 0);
        assert_eq!(outcome.translations[7].as_ref().ok(), Some(&"T7".to_string()));
        assert_eq!((outcome.requests, outcome.retries), (2, 1));
        let calls = calls.lock().unwrap();
        assert_eq!(calls[1], r#"[{"i":7,"t":"source 7"}]"#, "重发只带没译成的那一条，不带上下文");
    }

    #[test]
    fn an_id_that_stays_missing_is_a_visible_partial_failure_and_everything_else_is_kept() {
        let calls = Mutex::new(Vec::new());
        let source = texts(12);
        let outcome = run(&source, true, model(&calls, |_, id| id == 7 || id == 8));
        assert_eq!(outcome.failed(), 2);
        assert_eq!(outcome.translations[7], Err(Why::Missing));
        assert_eq!(outcome.translations[8], Err(Why::Missing));
        assert_eq!(outcome.translations[6].as_ref().ok(), Some(&"T6".to_string()));
        assert_eq!((outcome.requests, outcome.retries), (2, 1), "只重发一次");
        assert_eq!(outcome.main_reason(), Some(&Why::Missing));
        assert!(outcome.main_reason().unwrap().describe().contains("没有给出"));
    }

    #[test]
    fn a_request_error_hits_only_its_own_chunk_and_is_reported_after_one_more_try() {
        let source = texts(60);
        let asked = AtomicUsize::new(0);
        let ask = |user: String| {
            asked.fetch_add(1, Ordering::SeqCst);
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            if items[0]["i"].as_i64().unwrap() >= 28 {
                return ready(Err("请求太频繁了，稍等再试".to_string()));
            }
            let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
            ready(Ok(Reply { text: Value::Array(all).to_string(), truncated: false }))
        };
        let outcome = run(&source, true, ask);
        // 第一块成功了，说明网络和 Key 都没问题：出错的那块再整块问一次（再错就如实报告），没有第三次
        assert_eq!(asked.load(Ordering::SeqCst), 3);
        assert_eq!(outcome.failed(), 30);
        assert!(outcome.translations[..30].iter().all(Result::is_ok));
        assert_eq!(outcome.translations[30], Err(Why::Request("请求太频繁了，稍等再试".into())));
        assert_eq!(outcome.main_reason().unwrap().describe(), "请求太频繁了，稍等再试");
    }

    #[test]
    fn an_unreadable_reply_is_retried_once_then_everything_is_reported_failed() {
        let asked = AtomicUsize::new(0);
        let ask = |_: String| {
            asked.fetch_add(1, Ordering::SeqCst);
            ready(Ok(Reply { text: "对不起，我不能翻译这个。".into(), truncated: false }))
        };
        let outcome = run(&texts(5), true, ask);
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(outcome.failed(), 5);
        assert!(matches!(outcome.main_reason(), Some(Why::Unreadable(_))));
    }

    #[test]
    fn conflicting_duplicates_are_failures_and_a_clean_retry_repairs_them() {
        let calls = AtomicUsize::new(0);
        let ask = |user: String| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let mut out: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": format!("T{}", item["i"])})).collect();
            if call == 1 {
                out.push(serde_json::json!({"i": 2, "t": "另一种译法"}));
            }
            ready(Ok(Reply { text: Value::Array(out).to_string(), truncated: false }))
        };
        let outcome = run(&texts(5), true, ask);
        assert_eq!(outcome.failed(), 0);
        assert_eq!(outcome.translations[2].as_ref().ok(), Some(&"T2".to_string()));
        assert_eq!((outcome.requests, outcome.retries), (2, 1));
    }

    #[test]
    fn an_empty_translation_is_a_failure_not_a_blank_success() {
        let ask = |user: String| {
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let out: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": if item["i"] == 1 { "  " } else { "ok" }})).collect();
            ready(Ok(Reply { text: Value::Array(out).to_string(), truncated: false }))
        };
        let outcome = run(&texts(3), true, ask);
        assert_eq!(outcome.translations[1], Err(Why::Empty));
        assert_eq!((outcome.requests, outcome.retries), (2, 1));
    }

    #[test]
    fn a_truncated_reply_keeps_what_arrived_and_the_rest_is_asked_again() {
        let calls = AtomicUsize::new(0);
        let ask = |user: String| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let all: Vec<String> = items.iter().map(|item| format!("{{\"i\":{},\"t\":\"T{}\"}}", item["i"], item["i"])).collect();
            let text = if call == 1 { format!("[{},{{\"i\":4,\"t\":\"T", all[..4].join(",")) } else { format!("[{}]", all.join(",")) };
            ready(Ok(Reply { text, truncated: call == 1 }))
        };
        let outcome = run(&texts(8), true, ask);
        assert_eq!(outcome.failed(), 0);
        assert_eq!((outcome.requests, outcome.retries), (2, 1));
        // 重发还是被截断时，说清楚是截断，而不是「模型没给」
        let ask = |_: String| ready(Ok(Reply { text: "[{\"i\":0,\"t\":\"T0\"},{\"i\":1,\"t\":\"T".into(), truncated: true }));
        let outcome = run(&texts(4), true, ask);
        assert_eq!(outcome.translations[0], Ok("T0".to_string()));
        assert_eq!(outcome.translations[1], Err(Why::Truncated));
        assert_eq!(outcome.translations[3], Err(Why::Truncated));
    }

    #[test]
    fn the_main_reason_prefers_what_the_user_can_act_on_then_the_most_common() {
        let outcome = Outcome {
            translations: vec![Err(Why::Missing), Err(Why::Missing), Err(Why::Empty), Err(Why::Request("Key 无效".into())), Ok("x".into())],
            requests: 0,
            retries: 0,
        };
        assert_eq!(outcome.main_reason(), Some(&Why::Request("Key 无效".into())));
        let outcome = Outcome { translations: vec![Err(Why::Empty), Err(Why::Missing), Err(Why::Missing)], requests: 0, retries: 0 };
        assert_eq!(outcome.main_reason(), Some(&Why::Missing));
        let outcome = Outcome { translations: vec![Ok("x".into())], requests: 0, retries: 0 };
        assert_eq!(outcome.main_reason(), None);
    }

    #[test]
    fn chunks_go_out_together_for_cloud_providers_and_one_at_a_time_for_local_ones() {
        for (parallel, expect_overlap) in [(true, true), (false, false)] {
            let running = AtomicUsize::new(0);
            let peak = AtomicUsize::new(0);
            let ask = |user: String| {
                let (running, peak) = (&running, &peak);
                async move {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    let items: Vec<Value> = serde_json::from_str(&user).unwrap();
                    let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
                    Ok(Reply { text: Value::Array(all).to_string(), truncated: false })
                }
            };
            let outcome = run(&texts(90), parallel, ask);
            assert_eq!(outcome.failed(), 0);
            assert_eq!(peak.load(Ordering::SeqCst) > 1, expect_overlap, "parallel={parallel}");
        }
    }

    // ---- 对齐粗检 ----

    /// 真实数据（MDN 中文 CSS 页第 43–54 块，deepseek-chat）：原文、老协议下整体错位的译文、对齐的译文。
    const MDN_SOURCE: [&str; 12] = [
        ">",
        "decorations",
        "＞盒对齐",
        ">盒模型",
        "在CSS2.1之后，规范的制定范围不断地增加，并且不同的CSS 模块进展差异如此之大，以至于按",
        "照模块来单独进行开发并发布建议变得更加有效。W3C现在不再对CSS 规范进行版本控制，而",
        "> Box sizing",
        "是定期对CSS规范的最新稳定状态凶和各个模块的进展用快照来记录。CSS模块现在有版本号或者",
        "Control Your LLM",
        ">层叠",
        "Infrastructure",
        "级别，例如CSS颜色模块第五版区。",
    ];
    const MDN_SHIFTED: [&str; 12] = [
        ">",
        "decorations",
        "> Box alignment",
        "> Box model",
        "After CSS2.1, the scope of the specification continued to expand, and the progress of different CSS modules varied so greatly that it became more effective to",
        "> Box sizing",
        "develop and publish recommendations for each module separately. The W3C no longer versions the CSS specification, but",
        "> Cascading",
        "instead periodically records snapshots of the latest stable state of the CSS specification and the progress of each module. CSS modules now have version numbers or",
        "> Control Your LLM",
        "> Cascading variables",
        "Infrastructure",
    ];
    const MDN_ALIGNED: [&str; 12] = [
        ">",
        "decorations",
        "> Box alignment",
        "> Box model",
        "After CSS2.1, the scope of specifications kept expanding, and the progress of different CSS modules varied so much that it became",
        "more effective to develop and publish recommendations per module. The W3C no longer versions the CSS specification, but",
        "> Box sizing",
        "instead periodically records snapshots of the latest stable state of the CSS specification and the progress of each module. CSS modules now have version numbers or",
        "Control Your LLM",
        "> Cascade",
        "Infrastructure",
        "levels, such as CSS Color Module Level 5.",
    ];

    fn some(items: &[&'static str]) -> Vec<Option<&'static str>> {
        items.iter().map(|item| Some(*item)).collect()
    }

    #[test]
    fn the_length_profile_flags_a_real_one_slot_shift_and_leaves_the_aligned_reply_alone() {
        assert!(suspicious_windows(&MDN_SOURCE, &some(&MDN_ALIGNED), true).is_empty(), "对齐的译文不能被判可疑");
        let shifted = suspicious_windows(&MDN_SOURCE, &some(&MDN_SHIFTED), true);
        assert!(!shifted.is_empty(), "整体错了一位的译文必须被查出来");
        assert!(shifted.iter().all(|start| start + CHECK_WINDOW <= MDN_SOURCE.len()));
    }

    #[test]
    fn the_check_needs_complete_neighbours_and_enough_signal() {
        // 窗口里有没译成的块 / 贴着边界：跳过，不猜
        let mut partial = some(&MDN_SHIFTED);
        partial[5] = None;
        let with_gap = suspicious_windows(&MDN_SOURCE, &partial, true);
        // 起点 a 的窗口要用到 a−1 ..= a+5 这些位置的译文；第 5 块没有译文，所以 a ≤ 6 的窗口都跳过
        assert!(with_gap.iter().all(|start| *start > 6), "{with_gap:?}");
        // 少于一个窗口：没法查
        assert!(suspicious_windows(&MDN_SOURCE[..4], &some(&MDN_SHIFTED[..4]), true).is_empty());
        // 中英以外的目标语言没量过长度比：即使错位明显也不报，宁可漏检
        assert!(!suspicious_windows(&MDN_SOURCE, &some(&MDN_SHIFTED), true).is_empty());
        assert!(suspicious_windows(&MDN_SOURCE, &some(&MDN_SHIFTED), Direction::Other).is_empty());
        // 长度全都差不多（一列菜单项）：错位也看不出来，不能因此乱报
        let menu = ["Home", "Docs", "Blog", "Help", "Shop", "Cart", "Jobs", "Team"];
        let zh = ["首页", "文档", "博客", "帮助", "商店", "购物", "招聘", "团队"];
        assert!(suspicious_windows(&menu, &some(&zh), false).is_empty());
        let rotated = ["团队", "首页", "文档", "博客", "帮助", "商店", "购物", "招聘"];
        assert!(suspicious_windows(&menu, &some(&rotated), false).is_empty());
    }

    #[test]
    fn text_the_model_left_unchanged_never_looks_misaligned() {
        // 命令、版本号、专有名词原样保留：译文 = 原文，长度失配在两个方向上都小于「本位平均失配」的门槛，一整页都原样也不会被判可疑
        let lines = [
            "git commit -m \"fix\"", "v5.9.3", "Tauri", "npm run build && npm test -- --watch", "OK", "README.md",
            "https://github.com/tauri-apps/tauri/blob/dev/crates/tauri/src/lib.rs", "x", "Rust", "cargo test --offline", "2026-10-31", "SENSE",
        ];
        for to_english in [true, false] {
            assert!(suspicious_windows(&lines, &some(&lines), to_english).is_empty(), "to_english={to_english}");
        }
    }

    #[test]
    fn weights_count_han_as_one_and_latin_and_digits_as_half() {
        assert_eq!(weight("你好"), 2.0);
        assert_eq!(weight("ab1"), 1.5);
        assert!((weight("a b") - 1.3).abs() < 1e-9);
        assert_eq!(weight(""), 0.0);
    }

    /// 原文：一长一短交替的英文；译成中文时汉字数按权重比折算。
    fn varied_sources(n: usize) -> Vec<String> {
        (0..n)
            .map(|k| if k % 2 == 0 { format!("Menu {k}") } else { format!("This is a longer paragraph of running text number {k} that goes on for a while to make it long") })
            .collect()
    }

    fn zh_for(source: &str) -> String {
        "字".repeat(((weight(source) * RATIO_TO_CHINESE).round() as usize).max(1))
    }

    /// 假模型：`shift(第几次调用, 这条消息里第一个编号)` 为真的那几次调用把译文错一位（编号照抄、内容是下一条的译文），其余调用对齐。
    fn shifting_model<'a>(source: &'a [String], calls: &'a AtomicUsize, shift: impl Fn(usize, usize) -> bool + 'a) -> impl Fn(String) -> std::future::Ready<Result<Reply, String>> + 'a {
        move |user: String| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let shifted = shift(call, items[0]["i"].as_u64().unwrap() as usize);
            let out: Vec<Value> = items
                .iter()
                .map(|item| {
                    let id = item["i"].as_u64().unwrap() as usize;
                    let from = if shifted { (id + 1).min(source.len() - 1) } else { id };
                    serde_json::json!({"i": id, "t": zh_for(&source[from])})
                })
                .collect();
            ready(Ok(Reply { text: Value::Array(out).to_string(), truncated: false }))
        }
    }

    #[test]
    fn a_shifted_reply_is_asked_again_as_a_whole_and_the_second_reply_replaces_it() {
        let source = varied_sources(16);
        let calls = AtomicUsize::new(0);
        let outcome = run(&source, true, shifting_model(&source, &calls, |call, _| call == 1));
        assert_eq!((outcome.requests, outcome.retries), (2, 1));
        assert_eq!(outcome.failed(), 0);
        for (k, item) in outcome.translations.iter().enumerate() {
            assert_eq!(item.as_ref().unwrap(), &zh_for(&source[k]), "第 {k} 块用的是对齐的第二次回复");
        }
    }

    #[test]
    fn a_reply_that_stays_shifted_is_demoted_as_a_whole_chunk_not_shown_as_success() {
        let source = varied_sources(16);
        let calls = AtomicUsize::new(0);
        let outcome = run(&source, true, shifting_model(&source, &calls, |call, _| call <= 2));
        assert_eq!((outcome.requests, outcome.retries), (2, 1), "只重发一次，不无限重试");
        // 可疑窗口只覆盖错位的开头，错位多半延续到块尾：二次仍可疑，整块都不能当成功
        assert_eq!(outcome.failed(), 16);
        assert!(outcome.translations.iter().all(|item| *item == Err(Why::Misaligned)));
        assert_eq!(outcome.main_reason(), Some(&Why::Misaligned));
        assert!(outcome.main_reason().unwrap().describe().contains("错了位"));
    }

    #[test]
    fn a_shifted_reply_followed_by_a_network_error_falls_back_to_the_first_reply_and_is_demoted_whole() {
        let source = varied_sources(16);
        let calls = AtomicUsize::new(0);
        let inner = shifting_model(&source, &calls, |call, _| call == 1);
        let ask = |user: String| {
            if calls.load(Ordering::SeqCst) >= 1 {
                calls.fetch_add(1, Ordering::SeqCst);
                return ready(Err("网络断了".to_string()));
            }
            inner(user)
        };
        let outcome = run(&source, true, ask);
        assert_eq!((outcome.requests, outcome.retries), (2, 1));
        assert_eq!(outcome.failed(), 16, "第一次的回复可疑、重发又断网：这一块的译文不能当成功");
        assert!(outcome.translations.iter().all(|item| *item == Err(Why::Misaligned)));
    }

    #[test]
    fn a_shift_in_one_chunk_does_not_take_the_other_chunks_down_with_it() {
        let source = varied_sources(60);
        // 第一块只有第一次回复错位，重发对齐；第二块一直对齐
        let calls = AtomicUsize::new(0);
        let outcome = run(&source, true, shifting_model(&source, &calls, |call, _| call == 1));
        assert_eq!(outcome.failed(), 0);
        for (k, item) in outcome.translations.iter().enumerate() {
            assert_eq!(item.as_ref().unwrap(), &zh_for(&source[k]), "第 {k} 块");
        }
        // 第一块每次都错位（重发也是）：只有这一块整块降级
        let calls = AtomicUsize::new(0);
        let outcome = run(&source, true, shifting_model(&source, &calls, |_, first_id| first_id < 28));
        assert_eq!(outcome.failed(), 30, "第一块重发后仍然错位：只有这一块整块降级");
        assert!(outcome.translations[..30].iter().all(|item| *item == Err(Why::Misaligned)));
        assert!(outcome.translations[30..].iter().all(Result::is_ok));
    }

    #[test]
    fn an_aligned_reply_triggers_no_extra_request() {
        let source = varied_sources(16);
        let calls = AtomicUsize::new(0);
        let outcome = run(&source, true, shifting_model(&source, &calls, |_, _| false));
        assert_eq!((outcome.requests, outcome.retries, outcome.failed()), (1, 0, 0));
    }

    #[test]
    fn a_chunk_that_failed_with_a_request_error_is_asked_again_when_the_other_chunks_succeeded() {
        let source = texts(60);
        let calls = AtomicUsize::new(0);
        let ask = |user: String| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            // 第二块（编号 30 起）第一次被限流
            if items[0]["i"].as_i64().unwrap() >= 28 && call <= 2 {
                return ready(Err("请求太频繁了，稍等再试".to_string()));
            }
            let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
            ready(Ok(Reply { text: Value::Array(all).to_string(), truncated: false }))
        };
        let outcome = run(&source, true, ask);
        assert_eq!(outcome.failed(), 0, "限流的那块再问一次就好了");
        assert_eq!((outcome.requests, outcome.retries), (3, 1));
    }

    #[test]
    fn when_every_chunk_fails_with_a_request_error_nothing_is_asked_again() {
        let asked = AtomicUsize::new(0);
        let ask = |_: String| {
            asked.fetch_add(1, Ordering::SeqCst);
            ready(Err("还没填 API Key".to_string()))
        };
        let outcome = run(&texts(90), true, ask);
        assert_eq!(asked.load(Ordering::SeqCst), 3, "三块各问一次，一个都没成功就不再问");
        assert_eq!((outcome.failed(), outcome.retries), (90, 0));
        assert_eq!(outcome.main_reason().unwrap().describe(), "还没填 API Key");
    }

    #[test]
    fn at_most_max_parallel_requests_are_in_flight_however_many_chunks_there_are() {
        let running = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let ask = |user: String| {
            let (running, peak) = (&running, &peak);
            async move {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }
                running.fetch_sub(1, Ordering::SeqCst);
                let items: Vec<Value> = serde_json::from_str(&user).unwrap();
                let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
                Ok(Reply { text: Value::Array(all).to_string(), truncated: false })
            }
        };
        let outcome = run(&texts(300), true, ask);
        assert_eq!(outcome.failed(), 0);
        assert_eq!(outcome.requests, 10, "300 条 = 10 块");
        // 用字面量 4 而不是常量本身：把常量改大改小，这里要红（并发数是按 provider 的限流定的，改之前要想清楚）
        assert_eq!(peak.load(Ordering::SeqCst), 4, "同时在飞的请求数");
    }

    #[test]
    fn nothing_to_translate_asks_nothing() {
        let calls = Mutex::new(Vec::new());
        let outcome = run(&[], true, model(&calls, |_, _| false));
        assert!(outcome.translations.is_empty());
        assert_eq!((outcome.requests, outcome.retries), (0, 0));
        assert!(calls.lock().unwrap().is_empty());
    }

    // ---- 评审补充（第一轮）：下标基准、粗检方向、各个分支、被钉住的常量 ----

    /// 要「长短不一、不按奇偶交替」的数据，又不能每次跑都不一样：线性同余，种子固定。
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// 一屏原文：长短不一，一半的时候沿用上一条的量级（真实页面里连着的几行常常差不多长）。
    /// 译成中文时原文是英文（一个 x 权重 0.5），译成英文时原文是中文（一个「字」权重 1）。
    fn random_sources(rng: &mut Lcg, n: usize, to_english: bool) -> Vec<String> {
        let mut len = 30.0_f64;
        (0..n)
            .map(|_| {
                if rng.next() < 0.5 {
                    len = 4.0 * 30.0_f64.powf(rng.next());
                } else {
                    len *= 0.8 + 0.4 * rng.next();
                }
                let len = len.round().clamp(2.0, 140.0) as usize;
                if to_english { "字".repeat(len / 2 + 1) } else { "x".repeat(len) }
            })
            .collect()
    }

    /// 对齐的译文：权重 = 原文权重 × 这个方向的典型长度比 × 噪声（对数均匀，±`noise`）。
    /// 长度比用实测的字面值（1.57 / 0.65），故意不引用 `RATIO_*`：常量被改了，这里的数据不能跟着变。
    fn translate_like(source: &str, to_english: bool, noise: f64, rng: &mut Lcg) -> String {
        let ratio = if to_english { 1.57 } else { 0.65 };
        let wanted = weight(source) * ratio * ((2.0 * rng.next() - 1.0) * noise).exp();
        if to_english {
            "x".repeat(((wanted * 2.0).round() as usize).max(1))
        } else {
            "字".repeat((wanted.round() as usize).max(1))
        }
    }

    fn aligned_page(seed: u64, n: usize, to_english: bool, noise: f64) -> (Vec<String>, Vec<String>) {
        let mut rng = Lcg(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1));
        let source = random_sources(&mut rng, n, to_english);
        let translated = source.iter().map(|text| translate_like(text, to_english, noise, &mut rng)).collect();
        (source, translated)
    }

    fn windows_of(source: &[String], translated: &[String], to_english: bool) -> Vec<usize> {
        let source: Vec<&str> = source.iter().map(String::as_str).collect();
        let translated: Vec<Option<&str>> = translated.iter().map(|text| Some(text.as_str())).collect();
        suspicious_windows(&source, &translated, to_english)
    }

    #[test]
    fn the_check_uses_the_length_ratio_of_the_direction_it_is_given() {
        // 同一批对齐的译文：按它真正的方向查，一页都不会被判可疑；按反方向查（长度比差 2.4 倍），就有不少页被判可疑——所以方向必须传对
        for to_english in [false, true] {
            let (mut right, mut wrong) = (0, 0);
            for seed in 0..200 {
                let (source, translated) = aligned_page(seed, 14, to_english, 0.25);
                right += usize::from(!windows_of(&source, &translated, to_english).is_empty());
                wrong += usize::from(!windows_of(&source, &translated, !to_english).is_empty());
            }
            assert_eq!(right, 0, "按真正的方向查，对齐的页面不该被判可疑（to_english={to_english}）");
            assert!(wrong >= 12, "按反方向查应当失配：{wrong} / 200 页（to_english={to_english}）");
        }
    }

    #[test]
    fn a_one_slot_shift_is_found_whichever_way_it_goes() {
        for to_english in [false, true] {
            let (mut up, mut down) = (0, 0);
            for seed in 0..200 {
                let (source, translated) = aligned_page(seed, 14, to_english, 0.25);
                let n = translated.len();
                // 整体上移一位：第 k 条译文其实是第 k+1 条原文的译文；整体下移一位：第 k 条是第 k−1 条的
                let moved_up: Vec<String> = (0..n).map(|k| translated[(k + 1).min(n - 1)].clone()).collect();
                let moved_down: Vec<String> = (0..n).map(|k| translated[k.saturating_sub(1)].clone()).collect();
                up += usize::from(!windows_of(&source, &moved_up, to_english).is_empty());
                down += usize::from(!windows_of(&source, &moved_down, to_english).is_empty());
            }
            assert!(up >= 120, "上移一位：{up} / 200 页查出来（to_english={to_english}）");
            assert!(down >= 120, "下移一位：{down} / 200 页查出来（to_english={to_english}）");
        }
    }


    #[test]
    fn the_measured_constants_are_pinned() {
        // 这些数是量出来的（分块大小、并发、上下文；粗检的长度比、窗口、阈值，见 CHANGELOG 的表 4–7），改任何一个都要重新量，别顺手改
        assert_eq!((CHUNK_MAX, MAX_PARALLEL, CONTEXT), (30, 4, 2));
        assert_eq!((RATIO_TO_ENGLISH, RATIO_TO_CHINESE), (1.57, 0.65));
        assert_eq!((CHECK_WINDOW, CHECK_TAU, CHECK_MARGIN, CHECK_SMOOTH), (5, 0.55, 1.2, 2.0));
        assert_eq!(RETRY_PAUSE, Duration::from_millis(1500));
    }

    /// 出毛病的方式：对齐 / 错一位（编号照抄、内容是下一条的译文）/ 丢掉这个编号。
    #[derive(Clone, Copy, PartialEq)]
    enum Fault {
        None,
        Shift,
        Drop,
    }

    /// 每条原文的对齐译文（译成中文）：同一条每次问都是同样的。
    fn aligned_translations(source: &[String]) -> Vec<String> {
        let mut rng = Lcg(7);
        source.iter().map(|text| translate_like(text, false, 0.2, &mut rng)).collect()
    }

    /// 假模型：按原文长度回对齐的译文（编号照抄）。`fault(第几次调用, 编号)` 决定这一条出不出毛病；调用次数从 1 数。
    fn length_model<'a>(
        source: &'a [String],
        calls: &'a AtomicUsize,
        fault: impl Fn(usize, usize) -> Fault + 'a,
    ) -> impl Fn(String) -> std::future::Ready<Result<Reply, String>> + 'a {
        let aligned = aligned_translations(source);
        move |user: String| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let out: Vec<Value> = items
                .iter()
                .filter_map(|item| {
                    let id = item["i"].as_u64().unwrap() as usize;
                    match fault(call, id) {
                        Fault::Drop => None,
                        Fault::Shift => Some(serde_json::json!({"i": id, "t": aligned[(id + 1).min(aligned.len() - 1)]})),
                        Fault::None => Some(serde_json::json!({"i": id, "t": aligned[id]})),
                    }
                })
                .collect();
            ready(Ok(Reply { text: Value::Array(out).to_string(), truncated: false }))
        }
    }

    fn assert_every_block_has_its_own_translation(outcome: &Outcome, source: &[String]) {
        let aligned = aligned_translations(source);
        assert_eq!(outcome.translations.len(), source.len());
        for (k, item) in outcome.translations.iter().enumerate() {
            assert_eq!(item.as_ref().ok(), Some(&aligned[k]), "第 {k} 块");
        }
    }

    #[test]
    fn a_later_chunks_refilled_id_lands_on_its_own_block_and_nowhere_else() {
        // 70 条 → 24 / 23 / 23：第二块自己是 24..47、前面带 2 条上下文（从 22 起）。它丢了第 30 块，补缺的回复要落回第 30 块
        let source = random_sources(&mut Lcg(11), 70, false);
        let calls = AtomicUsize::new(0);
        let outcome = run(&source, true, length_model(&source, &calls, |call, id| if call == 2 && id == 30 { Fault::Drop } else { Fault::None }));
        assert_eq!((outcome.requests, outcome.retries, outcome.failed()), (4, 1, 0));
        assert_every_block_has_its_own_translation(&outcome, &source);
    }

    #[test]
    fn the_alignment_check_compares_each_chunk_with_its_own_sources() {
        let source = random_sources(&mut Lcg(11), 70, false);
        // 对齐的整屏：每块问一次就够，粗检不能因为拿错了原文（别的块的）而误报
        let calls = AtomicUsize::new(0);
        let outcome = run(&source, true, length_model(&source, &calls, |_, _| Fault::None));
        assert_eq!((outcome.requests, outcome.retries, outcome.failed()), (3, 0, 0));
        assert_every_block_has_its_own_translation(&outcome, &source);
        // 第二块（不是第一块）的第一次回复错了一位：要被查出来、整块重发，重发的对齐回复换上来
        let calls = AtomicUsize::new(0);
        let outcome = run(&source, true, length_model(&source, &calls, |call, _| if call == 2 { Fault::Shift } else { Fault::None }));
        assert_eq!((outcome.requests, outcome.retries, outcome.failed()), (4, 1, 0));
        assert_every_block_has_its_own_translation(&outcome, &source);
    }

    #[test]
    fn a_shift_that_only_shows_once_the_missing_id_is_filled_in_demotes_the_chunk_without_a_whole_resend() {
        // 第一次回复：第 5–11 块错一位，第 8 块没给。含第 8 块的窗口（它的译文和邻位）都被跳过，第一次检查什么也查不出来；
        // 补上第 8 块之后才查得出来。补缺已经是第二次请求，这一块直接整块降级，不再整块重发（第三次）
        let source = varied_sources(16);
        let calls = AtomicUsize::new(0);
        let ask = |user: String| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let out: Vec<Value> = items
                .iter()
                .filter_map(|item| {
                    let id = item["i"].as_u64().unwrap() as usize;
                    if call == 1 && id == 8 {
                        return None;
                    }
                    let from = if call == 1 && (5..=11).contains(&id) { id + 1 } else { id };
                    Some(serde_json::json!({"i": id, "t": zh_for(&source[from])}))
                })
                .collect();
            ready(Ok(Reply { text: Value::Array(out).to_string(), truncated: false }))
        };
        let outcome = run(&source, true, ask);
        assert_eq!((outcome.requests, outcome.retries), (2, 1));
        assert_eq!(outcome.failed(), 16, "补缺之后才看出错位：整块降级，不当成功");
        assert!(outcome.translations.iter().all(|item| *item == Err(Why::Misaligned)));
    }

    #[test]
    fn a_chunk_whose_retry_failed_is_not_asked_again_as_a_whole() {
        // 第一块：第一次回复读不出来，补问那次被限流；第二块一切正常。第一块的第一次请求是成功的（回复本身有问题），
        // 不再整块问：否则一块会发到 4 次请求
        let chunk0_calls = AtomicUsize::new(0);
        let total_calls = AtomicUsize::new(0);
        let ask = |user: String| {
            total_calls.fetch_add(1, Ordering::SeqCst);
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            if items[0]["i"].as_i64().unwrap() == 0 {
                return match chunk0_calls.fetch_add(1, Ordering::SeqCst) {
                    0 => ready(Ok(Reply { text: "对不起，我不能翻译这个。".into(), truncated: false })),
                    _ => ready(Err("请求太频繁了，稍等再试".to_string())),
                };
            }
            let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
            ready(Ok(Reply { text: Value::Array(all).to_string(), truncated: false }))
        };
        let outcome = run(&texts(60), true, ask);
        assert_eq!(total_calls.load(Ordering::SeqCst), 3, "第一块 2 次（第一次 + 补问）、第二块 1 次");
        assert_eq!((outcome.requests, outcome.retries), (3, 1));
        assert_eq!(outcome.failed(), 30);
        assert_eq!(outcome.translations[0], Err(Why::Request("请求太频繁了，稍等再试".into())));
        assert!(outcome.translations[30..].iter().all(Result::is_ok));
    }

    #[test]
    fn a_retry_that_fails_on_a_chunk_that_otherwise_worked_keeps_what_worked() {
        // 第一块丢了第 5 块，补问那次被限流：别的 29 块照旧，只有第 5 块报「请求失败」，不整块再问
        let total_calls = AtomicUsize::new(0);
        let ask = |user: String| {
            let call = total_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            if call == 2 {
                return ready(Err("请求太频繁了，稍等再试".to_string()));
            }
            let all: Vec<Value> = items
                .iter()
                .filter(|item| !(call == 1 && item["i"] == 5))
                .map(|item| serde_json::json!({"i": item["i"], "t": format!("T{}", item["i"])}))
                .collect();
            ready(Ok(Reply { text: Value::Array(all).to_string(), truncated: false }))
        };
        let outcome = run(&texts(60), true, ask);
        assert_eq!(total_calls.load(Ordering::SeqCst), 3);
        assert_eq!(outcome.failed(), 1);
        assert_eq!(outcome.translations[5], Err(Why::Request("请求太频繁了，稍等再试".into())));
        assert_eq!(outcome.translations[4], Ok("T4".to_string()));
        assert_eq!(outcome.translations[6], Ok("T6".to_string()));
    }

    #[test]
    fn the_re_ask_waits_before_it_goes_out() {
        // 被限流的块紧接着再发多半还是被限流：先等一等。测试里的运行时要开计时器
        let source = texts(60);
        let refs: Vec<&str> = source.iter().map(String::as_str).collect();
        let stamps = Mutex::new(Vec::<(i64, std::time::Instant)>::new());
        let ask = |user: String| {
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let first = items[0]["i"].as_i64().unwrap();
            let tries_of_this_chunk = {
                let mut seen = stamps.lock().unwrap();
                seen.push((first, std::time::Instant::now()));
                seen.iter().filter(|(id, _)| *id == first).count()
            };
            if first == 28 && tries_of_this_chunk == 1 {
                return ready(Err("请求太频繁了，稍等再试".to_string()));
            }
            let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
            ready(Ok(Reply { text: Value::Array(all).to_string(), truncated: false }))
        };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let outcome = runtime.block_on(translate_all_with(&refs, true, false, Duration::from_millis(150), ask));
        assert_eq!(outcome.failed(), 0);
        let seen = stamps.lock().unwrap();
        let mut tries = seen.iter().filter(|(id, _)| *id == 28).map(|(_, at)| *at);
        let (first_try, second_try) = (tries.next().unwrap(), tries.next().expect("被限流的那块要再问一次"));
        assert!(second_try.duration_since(first_try) >= Duration::from_millis(140), "再问之前要等：{:?}", second_try.duration_since(first_try));
    }

    #[test]
    fn several_chunks_that_were_rate_limited_share_one_pause_before_they_are_asked_again() {
        // 90 条 = 3 块；第二、三块第一次都被限流：再问之前只等一次，不是每块各等一次
        let source = texts(90);
        let refs: Vec<&str> = source.iter().map(String::as_str).collect();
        let stamps = Mutex::new(Vec::<(i64, std::time::Instant)>::new());
        let ask = |user: String| {
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let first = items[0]["i"].as_i64().unwrap();
            let tries_of_this_chunk = {
                let mut seen = stamps.lock().unwrap();
                seen.push((first, std::time::Instant::now()));
                seen.iter().filter(|(id, _)| *id == first).count()
            };
            if first >= 28 && tries_of_this_chunk == 1 {
                return ready(Err("请求太频繁了，稍等再试".to_string()));
            }
            let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
            ready(Ok(Reply { text: Value::Array(all).to_string(), truncated: false }))
        };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let outcome = runtime.block_on(translate_all_with(&refs, true, false, Duration::from_millis(200), ask));
        assert_eq!(outcome.failed(), 0);
        let seen = stamps.lock().unwrap();
        let tries = |id: i64| seen.iter().filter(|(first, _)| *first == id).map(|(_, at)| *at).collect::<Vec<_>>();
        let (second, third) = (tries(28), tries(58));
        assert_eq!((second.len(), third.len()), (2, 2), "两块各问两次");
        assert!(second[1].duration_since(third[0]) >= Duration::from_millis(180), "第一块再问之前要等");
        assert!(third[1].duration_since(second[1]) < Duration::from_millis(100), "第二块再问不再等第二次：{:?}", third[1].duration_since(second[1]));
    }

    #[test]
    fn nothing_waits_when_no_chunk_needs_to_be_asked_again_as_a_whole() {
        // 等待只属于「第一次请求就失败、别的块成功」的整块再问：全部成功不等；补缺那一步被限流（第一次请求是成功的）也不等。
        // 把等待挪到再问循环之前、或者「只要有块成功就先等」，每个多块的整页都会多等一次
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let source = texts(90);
        let refs: Vec<&str> = source.iter().map(String::as_str).collect();
        let pause = Duration::from_millis(400);

        let calls = Mutex::new(Vec::new());
        let started = std::time::Instant::now();
        let outcome = runtime.block_on(translate_all_with(&refs, true, false, pause, model(&calls, |_, _| false)));
        assert_eq!(outcome.failed(), 0);
        assert!(started.elapsed() < Duration::from_millis(100), "全部成功不该等：{:?}", started.elapsed());

        let total_calls = AtomicUsize::new(0);
        let ask = |user: String| {
            let call = total_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            if call == 2 {
                return ready(Err("请求太频繁了，稍等再试".to_string()));
            }
            let all: Vec<Value> = items.iter().filter(|item| !(call == 1 && item["i"] == 5)).map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
            ready(Ok(Reply { text: Value::Array(all).to_string(), truncated: false }))
        };
        let started = std::time::Instant::now();
        let outcome = runtime.block_on(translate_all_with(&refs, true, false, pause, ask));
        assert_eq!(outcome.failed(), 1, "第 5 块的补问被限流，别的都在");
        assert!(started.elapsed() < Duration::from_millis(100), "补缺那步被限流不该等：{:?}", started.elapsed());
    }

    #[test]
    fn the_entry_the_app_calls_waits_the_documented_pause() {
        // 其余用例直接调 translate_all_with 并自带等待时长；应用真正调的是 translate_all，它传的必须是 RETRY_PAUSE（真等 1.5 秒）
        let source = texts(60);
        let refs: Vec<&str> = source.iter().map(String::as_str).collect();
        let stamps = Mutex::new(Vec::<(i64, std::time::Instant)>::new());
        let ask = |user: String| {
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let first = items[0]["i"].as_i64().unwrap();
            let tries_of_this_chunk = {
                let mut seen = stamps.lock().unwrap();
                seen.push((first, std::time::Instant::now()));
                seen.iter().filter(|(id, _)| *id == first).count()
            };
            if first == 28 && tries_of_this_chunk == 1 {
                return ready(Err("请求太频繁了，稍等再试".to_string()));
            }
            let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": "ok"})).collect();
            ready(Ok(Reply { text: Value::Array(all).to_string(), truncated: false }))
        };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let outcome = runtime.block_on(translate_all(&refs, true, false, ask));
        assert_eq!(outcome.failed(), 0);
        let seen = stamps.lock().unwrap();
        let mut tries = seen.iter().filter(|(id, _)| *id == 28).map(|(_, at)| *at);
        let (first_try, second_try) = (tries.next().unwrap(), tries.next().expect("被限流的那块要再问一次"));
        assert!(second_try.duration_since(first_try) >= RETRY_PAUSE - Duration::from_millis(100), "{:?}", second_try.duration_since(first_try));
    }

    #[test]
    fn a_whole_resend_is_the_same_message_as_the_first_request_context_included() {
        // 文档写的是「同一条消息」：整块重发带着一模一样的上下文，不是只带自己的那几条
        let source = random_sources(&mut Lcg(11), 70, false);
        let seen = Mutex::new(Vec::<String>::new());
        let calls = AtomicUsize::new(0);
        let inner = length_model(&source, &calls, |call, _| if call == 2 { Fault::Shift } else { Fault::None });
        let ask = |user: String| {
            seen.lock().unwrap().push(user.clone());
            inner(user)
        };
        let outcome = run(&source, true, ask);
        assert_eq!((outcome.requests, outcome.retries, outcome.failed()), (4, 1, 0));
        let seen = seen.lock().unwrap();
        // 第 1 次是第一块，第 2 次是第二块（错位），第 3 次是第二块的整块重发，第 4 次是第三块
        let first_ids = |user: &str| serde_json::from_str::<Vec<Value>>(user).unwrap().iter().map(|item| item["i"].as_i64().unwrap()).collect::<Vec<_>>();
        assert_eq!(seen[1], seen[2]);
        assert_eq!((first_ids(&seen[1]).first().copied(), first_ids(&seen[1]).last().copied()), (Some(22), Some(48)), "第二块自己 24..47，前后各 2 条上下文");
    }

    #[test]
    fn a_complete_reply_that_needed_the_lenient_scan_keeps_its_last_element() {
        // 只有被截断的回复才丢最后一个元素；完整的、只是走了宽松扫描的回复一个都不能少
        let ask = |_: String| ready(Ok(Reply { text: r#"[{"i":0","t":"甲"},{"i":1","t":"乙"},{"i":2","t":"丙"}]"#.into(), truncated: false }));
        let outcome = run(&texts(3), true, ask);
        assert_eq!(outcome.translations, vec![Ok("甲".to_string()), Ok("乙".to_string()), Ok("丙".to_string())]);
        assert_eq!((outcome.requests, outcome.retries), (1, 0));
    }

    #[test]
    fn a_slow_chunk_does_not_hold_back_the_chunks_after_it() {
        // 150 条 = 5 块。第 0 块特别慢：第 1–3 块早就完成了，第 4 块（自己 120 起，上下文从 118 起）不能等第 0 块结束才发出去
        let clock = AtomicUsize::new(0);
        let started = Mutex::new(HashMap::<i64, usize>::new());
        let finished = Mutex::new(HashMap::<i64, usize>::new());
        let ask = |user: String| {
            let (clock, started, finished) = (&clock, &started, &finished);
            async move {
                let items: Vec<Value> = serde_json::from_str(&user).unwrap();
                let first = items[0]["i"].as_i64().unwrap();
                started.lock().unwrap().insert(first, clock.fetch_add(1, Ordering::SeqCst));
                for _ in 0..if first == 0 { 60 } else { 1 } {
                    tokio::task::yield_now().await;
                }
                finished.lock().unwrap().insert(first, clock.fetch_add(1, Ordering::SeqCst));
                let all: Vec<Value> = items.iter().map(|item| serde_json::json!({"i": item["i"], "t": format!("T{}", item["i"])})).collect();
                Ok(Reply { text: Value::Array(all).to_string(), truncated: false })
            }
        };
        let outcome = run(&texts(150), true, ask);
        let (started, finished) = (started.lock().unwrap(), finished.lock().unwrap());
        assert!(started[&118] < finished[&0], "第 4 块在第 0 块结束之前就该发出去：开始 {} / 第 0 块结束 {}", started[&118], finished[&0]);
        // 乱序完成也不能串位
        for k in 0..150 {
            assert_eq!(outcome.translations[k].as_ref().ok(), Some(&format!("T{k}")), "第 {k} 块");
        }
    }

    #[test]
    fn a_half_finished_last_element_of_a_truncated_reply_is_never_shown_as_a_translation() {
        let calls = AtomicUsize::new(0);
        let ask = |user: String| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            let items: Vec<Value> = serde_json::from_str(&user).unwrap();
            let all: Vec<String> = items.iter().map(|item| format!("{{\"i\":{},\"t\":\"T{}\"}}", item["i"], item["i"])).collect();
            let text = if call == 1 {
                // 最后一条被截断在译文中间，而译文里恰好有 `"}`：宽松扫描会把它当成这个元素的收尾
                format!("[{},{{\"i\":3,\"t\":\"调用 f({{\\\"a\\\":\\\"b\\\"}}) 之后再", all[..3].join(","))
            } else {
                format!("[{}]", all.join(","))
            };
            ready(Ok(Reply { text, truncated: call == 1 }))
        };
        let outcome = run(&texts(5), true, ask);
        assert_eq!(outcome.translations[3], Ok("T3".to_string()), "半截的那条要换成补问回来的整句，不能留着半句话");
        assert_eq!(outcome.translations[4], Ok("T4".to_string()));
        assert_eq!((outcome.requests, outcome.retries, outcome.failed()), (2, 1, 0));
    }

    #[test]
    fn an_unreadable_reply_that_hit_the_token_limit_is_reported_as_truncated_not_as_unreadable() {
        let ask = |_: String| ready(Ok(Reply { text: "好的，下面是译文：".into(), truncated: true }));
        let outcome = run(&texts(3), true, ask);
        assert!(outcome.translations.iter().all(|item| *item == Err(Why::Truncated)));
        let ask = |_: String| ready(Ok(Reply { text: "好的，下面是译文：".into(), truncated: false }));
        let outcome = run(&texts(3), true, ask);
        assert!(matches!(outcome.translations[0], Err(Why::Unreadable(_))));
    }

    #[test]
    fn among_reasons_of_the_same_kind_the_most_common_wins_and_a_tie_goes_to_the_first_seen() {
        let request = |text: &str| Err(Why::Request(text.to_string()));
        let outcome = Outcome { translations: vec![request("a"), request("b"), request("b")], requests: 0, retries: 0 };
        assert_eq!(outcome.main_reason(), Some(&Why::Request("b".into())));
        let outcome = Outcome { translations: vec![request("a"), request("b")], requests: 0, retries: 0 };
        assert_eq!(outcome.main_reason(), Some(&Why::Request("a".into())));
    }

    // ---- 解析与对齐的各个分支 ----

    #[test]
    fn brackets_in_the_wrong_order_are_unreadable_not_a_panic() {
        for raw in ["] 没有 [", "]", "] [", "[ ] ]x[", "x ] y [ z"] {
            let _ = parse_reply(raw);
        }
        assert!(parse_reply("] 没有 [").is_err());
    }

    #[test]
    fn an_id_with_a_fraction_is_not_an_id_but_a_whole_number_written_as_a_float_is() {
        assert!(parse_reply(r#"[{"i":1.5,"t":"x"}]"#).is_err());
        assert_eq!(ids(&parse_reply(r#"[{"i":2.0,"t":"y"}]"#).unwrap()), vec![(2, "y".into())]);
    }

    #[test]
    fn a_negative_id_is_a_recognised_stray_element_not_something_the_previous_one_swallows() {
        // 第一个元素的编号后面多了引号，只能走宽松扫描；中间那个负编号是被编造出来的，不能把前一条吞掉
        let parsed = parse_reply(r#"[{"i":0","t":"甲"},{"i":-3,"t":"x"},{"i":1,"t":"乙"}]"#).unwrap();
        assert_eq!(ids(&parsed), vec![(0, "甲".into()), (-3, "x".into()), (1, "乙".into())]);
        assert!(parsed.dropped.is_empty());
        let (slots, stray) = align(&parsed.entries, &[0, 1]);
        assert_eq!((slots, stray), (vec![Slot::Ok("甲".into()), Slot::Ok("乙".into())], 1));
    }

    #[test]
    fn json_debris_in_a_translation_is_caught_by_either_the_key_or_the_boundary_check() {
        // 只有「键」能认出来：下一个元素头认不出（键没加引号）、前面还漏了逗号，上一条把它吞了进去
        let parsed = parse_reply(r#"[{"i":0,"t":"甲"}{i:2,"t":"丙"}]"#).unwrap_err();
        assert!(parsed.contains("认不出"), "整段只有一个元素，它被判为带碎片丢掉，所以什么也没捞到：{parsed}");
        let parsed = parse_reply(r#"[{"i":0,"t":"甲"}{i:2,"t":"丙"},{"i":1,"t":"乙"}]"#).unwrap();
        assert_eq!(ids(&parsed), vec![(1, "乙".into())]);
        assert_eq!(parsed.dropped, vec![0]);
        // 只有「边界」能认出来：被吞进来的是别的键（x、t2）的元素，没有 i / t 这样的键
        let parsed = parse_reply(r#"[{"i":0,"t":"甲"},{"x":1,"t2":"y"},{"i":1,"t":"乙"}]"#).unwrap();
        assert_eq!(ids(&parsed), vec![(1, "乙".into())]);
        assert_eq!(parsed.dropped, vec![0]);
    }

    #[test]
    fn the_hand_written_decoder_handles_unicode_escapes_surrogate_pairs_and_newlines() {
        // 译文里有没转义的引号，严格解析过不去，走手写的解码：\u00e9、代理对 \ud83d\ude00、\n 都要认
        let parsed = parse_reply(r#"[{"i":0,"t":"a" b \u00e9 \ud83d\ude00 x\ny"},{"i":1,"t":"z"}]"#).unwrap();
        assert_eq!(parsed.entries[0].t, "a\" b é 😀 x\ny");
        assert!(parsed.entries[0].lenient);
        // 坏的转义（\u 后面不是 4 位十六进制）原样留下
        let parsed = parse_reply(r#"[{"i":0,"t":"q" \u12g4 \x"}]"#).unwrap();
        assert_eq!(parsed.entries[0].t, "q\" \\u12g4 \\x");
    }

    #[test]
    fn a_fence_with_a_language_tag_or_without_one_is_stripped() {
        assert_eq!(strip_fence("```json\n[1]\n```"), "[1]");
        assert_eq!(strip_fence("```\n[1]\n```"), "[1]");
        assert_eq!(strip_fence("  [1]  "), "[1]");
    }

    #[test]
    fn two_empty_translations_of_the_same_id_are_still_just_empty() {
        let (slots, _) = align(&[entry(0, ""), entry(0, "  "), entry(1, ""), entry(1, "乙")], &[0, 1]);
        assert_eq!(slots, vec![Slot::Empty, Slot::Conflict], "两次都是空的还是「空」；一空一非空才是「两种译文」");
    }
}
