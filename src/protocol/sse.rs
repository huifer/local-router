//! Server-sent events: the framing every streaming protocol ends up in.

/// One frame of an SSE stream.
#[derive(Debug, Clone)]
pub struct SseFrame {
    /// The `event:` name, when the protocol uses one.
    pub event: Option<String>,
    /// The `data:` payload.
    pub data: String,
}

impl SseFrame {
    pub fn data(data: impl Into<String>) -> Self {
        SseFrame {
            event: None,
            data: data.into(),
        }
    }

    pub fn named(event: impl Into<String>, data: impl Into<String>) -> Self {
        SseFrame {
            event: Some(event.into()),
            data: data.into(),
        }
    }

    /// The bytes to write on the wire.
    pub fn encode(&self) -> String {
        let mut out = String::new();
        if let Some(name) = &self.event {
            out.push_str("event: ");
            out.push_str(name);
            out.push('\n');
        }
        for line in self.data.split('\n') {
            out.push_str("data: ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
        out
    }
}

/// A sentinel some OpenAI-compatible servers send to close a stream.
pub const DONE: &str = "[DONE]";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_encode_with_event_and_data_lines() {
        let plain = SseFrame::data("{\"x\":1}");
        assert_eq!(plain.encode(), "data: {\"x\":1}\n\n");
        assert!(plain.event.is_none());

        let named = SseFrame::named("message_start", "{\"y\":2}");
        assert_eq!(named.encode(), "event: message_start\ndata: {\"y\":2}\n\n");
    }

    #[test]
    fn multi_line_data_becomes_one_data_prefix_per_line() {
        let frame = SseFrame::data("line one\nline two");
        assert_eq!(frame.encode(), "data: line one\ndata: line two\n\n");
    }

    #[test]
    fn the_done_sentinel_is_the_openai_close_marker() {
        assert_eq!(DONE, "[DONE]");
    }
}
