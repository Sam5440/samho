//! 最小 SSE（Server-Sent Events）解析器：字节流进、完整消息出。
//!
//! 只实现 TUI 需要的子集：`event:`/`data:`/`id:` 字段、`:注释` 忽略、
//! 多行 `data` 以 \n 连接、`\n` 与 `\r\n` 两种行尾。`retry` 字段忽略
//! （重连退避由客户端自己控制）。按行缓冲字节再解码，跨 chunk 切断的
//! 多字节 UTF-8 序列不会损坏（\n 不会出现在多字节序列内部）。

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SseMessage {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    /// 跨 chunk 的未完成消息行（行缓冲必须持久，不能每次 feed 重置）
    pending: Vec<String>,
}

impl SseParser {
    /// 喂入一个字节块，返回其中已完成的消息（按出现顺序）。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseMessage> {
        self.buffer.extend_from_slice(chunk);
        let mut messages = Vec::new();
        // 找下一行结尾（\n 或 \r\n 或 \r）
        while let Some(rel) = find_line_end(&self.buffer) {
            let (line, consumed) = split_line(&self.buffer, rel);
            self.buffer.drain(..consumed);
            if line.is_empty() {
                if !self.pending.is_empty() {
                    if let Some(message) = assemble(&self.pending) {
                        messages.push(message);
                    }
                    self.pending.clear();
                }
                continue;
            }
            self.pending.push(line);
        }
        messages
    }
}

/// 在字节缓冲里找下一个行终止符的相对位置（指向 \n 或 \r）。
fn find_line_end(buffer: &[u8]) -> Option<usize> {
    buffer.iter().position(|&b| b == b'\n' || b == b'\r')
}

/// 取出终止符前的行内容（已按 UTF-8 解码），返回 (行, 消耗字节数)。
fn split_line(buffer: &[u8], end: usize) -> (String, usize) {
    let bytes = &buffer[..end];
    let line = String::from_utf8_lossy(bytes).into_owned();
    let mut consumed = end + 1; // 跳过 \n 或 \r
    if buffer.get(end) == Some(&b'\r') && buffer.get(end + 1) == Some(&b'\n') {
        consumed += 1;
    }
    (line, consumed)
}

fn assemble(lines: &[String]) -> Option<SseMessage> {
    let mut event = String::new();
    let mut data: Vec<&str> = Vec::new();
    let mut id: Option<String> = None;
    for line in lines {
        if let Some(rest) = line.strip_prefix(':') {
            let _ = rest; // 注释
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_str(), ""),
        };
        match field {
            "event" => event = value.to_owned(),
            "data" => data.push(value),
            // 规范：id 含 NUL 时整条忽略
            "id" if !value.contains('\0') => id = Some(value.to_owned()),
            _ => {}
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(SseMessage {
        event: if event.is_empty() {
            "message".into()
        } else {
            event
        },
        data: data.join("\n"),
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(chunks: &[&[u8]]) -> Vec<SseMessage> {
        let mut parser = SseParser::default();
        let mut messages = Vec::new();
        for chunk in chunks {
            messages.extend(parser.feed(chunk));
        }
        messages
    }

    #[test]
    fn single_event_with_crlf_and_named_type() {
        let messages = parse_all(&[b"event: progress\r\ndata: {\"a\":1}\r\n\r\n"]);
        assert_eq!(
            messages,
            vec![SseMessage {
                event: "progress".into(),
                data: "{\"a\":1}".into(),
                id: None,
            }]
        );
    }

    #[test]
    fn chunk_boundaries_and_multibyte_split_do_not_corrupt() {
        // 「轨」的 UTF-8 为 3 字节，故意在第 2 字节处切开
        let full: Vec<u8> = "data: 轨迹事件\n\n".bytes().collect();
        let (a, b) = full.split_at(9);
        let messages = parse_all(&[a, b]);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].data, "轨迹事件");
    }

    #[test]
    fn multi_line_data_joins_with_newline_and_last_id_wins() {
        let messages = parse_all(&[b"id: 41\ndata: first\n", b"data: second\nid: 42\n\n"]);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].data, "first\nsecond");
        assert_eq!(messages[0].id.as_deref(), Some("42"));
    }

    #[test]
    fn comments_and_retry_and_unknown_fields_are_ignored() {
        let messages = parse_all(&[b": keep-alive\nretry: 3000\nfoo: bar\ndata: x\n\n"]);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].event, "message");
        assert_eq!(messages[0].data, "x");
    }

    #[test]
    fn data_only_dispatches_default_event_and_empty_events_are_skipped() {
        assert_eq!(parse_all(&[b"\n\n"]).len(), 0);
        assert_eq!(parse_all(&[b"id: 7\n\n"]).len(), 0);
        let messages = parse_all(&[b"data: y\n\n"]);
        assert_eq!(messages[0].event, "message");
    }

    #[test]
    fn incomplete_tail_is_buffered_until_complete() {
        let mut parser = SseParser::default();
        assert_eq!(parser.feed(b"event: trace\ndata: {\"seq\"").len(), 0);
        let messages = parser.feed(b":1}\n\n");
        assert_eq!(messages[0].data, "{\"seq\":1}");
        assert_eq!(messages[0].event, "trace");
    }
}
