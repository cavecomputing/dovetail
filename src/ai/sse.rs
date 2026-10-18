//! Server-sent events (SSE), the streaming format both request formats use: lines of `event:`
//! and `data:`, with a blank line ending each event. Small enough to parse by hand.

/// One server-sent event: its `event:` name (often empty) and its `data:` lines joined by `\n`.
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Splits a byte stream into server-sent events. Lines end in `\n` or `\r\n`; a blank line ends
/// an event; lines starting with `:` are comments (keep-alives); `id:` and `retry:` are ignored.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of a line not yet ended.
    partial: Vec<u8>,
    event: String,
    data: Option<String>,
}

impl SseParser {
    /// Takes the next bytes off the network and returns every event they complete.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.partial.extend_from_slice(bytes);
        let mut events = Vec::new();
        // A line is only read once its `\n` is here, so a character split between two chunks
        // is whole by then.
        while let Some(end) = self.partial.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line[..end]);
            events.extend(self.line(line.strip_suffix('\r').unwrap_or(&line)));
        }
        events
    }

    /// The stream has ended: returns the last event if no blank line came after it.
    pub fn finish(&mut self) -> Option<SseEvent> {
        let rest = std::mem::take(&mut self.partial);
        let rest = String::from_utf8_lossy(&rest);
        let rest = rest.strip_suffix('\r').unwrap_or(&rest);
        if rest.is_empty() {
            return self.line("");
        }
        self.line(rest).or_else(|| self.line(""))
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            let event = std::mem::take(&mut self.event);
            let data = self.data.take()?;
            return Some(SseEvent { event, data });
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match (field, &mut self.data) {
            ("event", _) => self.event = value.to_string(),
            ("data", Some(data)) => {
                data.push('\n');
                data.push_str(value);
            }
            ("data", None) => self.data = Some(value.to_string()),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::client::tests::{CHAT_STREAM, RESPONSES_STREAM};

    #[test]
    fn sse_splits_events_and_joins_data_lines() {
        let mut parser = SseParser::default();
        let events = parser.feed(
            b": comment\nevent: one\ndata: a\ndata:b\ndata\nid: 7\nretry: 10\n\ndata: second\n\n\n",
        );
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: "one".to_string(),
                    data: "a\nb\n".to_string(),
                },
                SseEvent {
                    event: String::new(),
                    data: "second".to_string(),
                },
            ]
        );
        assert_eq!(parser.finish(), None);
    }

    #[test]
    fn sse_handles_crlf_and_a_last_event_without_a_blank_line() {
        let mut parser = SseParser::default();
        let events = parser.feed(b"event: x\r\ndata: 1\r\n\r\ndata: 2\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "x");
        assert_eq!(events[0].data, "1");
        assert_eq!(parser.finish().unwrap().data, "2");

        let mut parser = SseParser::default();
        assert!(parser.feed(b"data: no newline at all").is_empty());
        assert_eq!(parser.finish().unwrap().data, "no newline at all");

        // An event name alone, or only comments, makes no event.
        let mut parser = SseParser::default();
        assert!(parser.feed(b"event: lonely\n\n: ping\n\n").is_empty());
        assert_eq!(parser.finish(), None);
    }

    #[test]
    fn sse_gives_the_same_events_whatever_the_chunk_boundaries() {
        for stream in [RESPONSES_STREAM, CHAT_STREAM] {
            let bytes = stream.as_bytes();
            let whole = SseParser::default().feed(bytes);
            for split in 0..bytes.len() {
                let mut parser = SseParser::default();
                let mut events = parser.feed(&bytes[..split]);
                events.extend(parser.feed(&bytes[split..]));
                assert_eq!(events, whole, "split at byte {split}");
            }
            // And one byte at a time, which splits the two-byte "ü" too.
            let mut parser = SseParser::default();
            let events: Vec<SseEvent> = bytes
                .iter()
                .flat_map(|byte| parser.feed(&[*byte]))
                .collect();
            assert_eq!(events, whole);
        }
    }
}
