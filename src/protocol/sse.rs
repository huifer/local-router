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
