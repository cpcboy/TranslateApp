//! 增量 SSE 解码器。
//!
//! 只做一件事：把字节流切成一个个 `data:` 负载。
//! 不假设分片边界与事件边界对齐，也不假设 UTF-8 字符不会被切开。

/// 事件分隔符长度，用于从缓冲区里切走已经完整的事件。
const LF_LF: &[u8] = b"\n\n";
const CRLF_CRLF: &[u8] = b"\r\n\r\n";

#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂一段字节，返回这一批里已经完整的事件负载。
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some((index, sep_len)) = find_boundary(&self.buf) {
            let raw: Vec<u8> = self.buf[..index].to_vec();
            self.buf.drain(..index + sep_len);
            if let Some(data) = parse_event(&raw) {
                events.push(data);
            }
        }

        events
    }

    /// 流结束时把残留内容当成最后一个事件（有的服务端不发结尾空行）。
    pub fn finish(&mut self) -> Vec<String> {
        if self.buf.is_empty() {
            return Vec::new();
        }
        let raw = std::mem::take(&mut self.buf);
        parse_event(&raw).into_iter().collect()
    }
}

fn find_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = find(buf, LF_LF).map(|index| (index, LF_LF.len()));
    let crlf = find(buf, CRLF_CRLF).map(|index| (index, CRLF_CRLF.len()));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 解析一个事件块：收集所有 `data:` 行，用换行拼接。
/// 注释行（`:` 开头，含心跳）与 `event:` / `id:` / `retry:` 一律忽略。
fn parse_event(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let mut parts: Vec<&str> = Vec::new();

    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        // SSE 规定：冒号后若有一个空格，去掉它（只去一个）。
        parts.push(rest.strip_prefix(' ').unwrap_or(rest));
    }

    if parts.is_empty() {
        return None;
    }
    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_event() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: hello\n\n");
        assert_eq!(events, vec!["hello".to_string()]);
    }

    #[test]
    fn event_split_across_chunks() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: hel").is_empty());
        assert!(decoder.push(b"lo\n").is_empty());
        let events = decoder.push(b"\n");
        assert_eq!(events, vec!["hello".to_string()]);
    }

    #[test]
    fn two_events_in_one_chunk() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: a\n\ndata: b\n\n");
        assert_eq!(events, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn multiline_data_is_joined_with_newline() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: line1\ndata: line2\n\n");
        assert_eq!(events, vec!["line1\nline2".to_string()]);
    }

    #[test]
    fn crlf_is_supported() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: x\r\n\r\n");
        assert_eq!(events, vec!["x".to_string()]);
    }

    #[test]
    fn comments_and_other_fields_are_ignored() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b": keep-alive\nevent: message\nid: 7\ndata: real\n\n");
        assert_eq!(events, vec!["real".to_string()]);
    }

    #[test]
    fn done_marker_passes_through() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: [DONE]\n\n");
        assert_eq!(events, vec!["[DONE]".to_string()]);
    }

    #[test]
    fn utf8_split_across_chunks_is_intact() {
        let payload = "data: 收费\n\n".as_bytes();
        let (head, tail) = payload.split_at(7); // 切在「收」的中间
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(head).is_empty());
        let events = decoder.push(tail);
        assert_eq!(events, vec!["收费".to_string()]);
    }

    #[test]
    fn finish_flushes_tail_without_blank_line() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: tail").is_empty());
        assert_eq!(decoder.finish(), vec!["tail".to_string()]);
        assert!(decoder.finish().is_empty(), "flush 之后不应重复输出");
    }
}
