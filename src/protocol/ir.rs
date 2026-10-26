//! The canonical, protocol-neutral shape of a chat request and reply.
//!
//! Every wire protocol the gateway speaks (OpenAI Chat Completions, OpenAI
//! Responses, Anthropic Messages, Gemini `generateContent`) is parsed *into*
//! these types, and every upstream provider is answered by formatting these
//! types back out. Anthropic Messages is the closest shape to a superset, so
//! the block model below follows it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which API shape a payload is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// `POST /v1/chat/completions`
    #[serde(rename = "openai_chat")]
    OpenAiChat,
    /// `POST /v1/responses`
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    /// `POST /v1/messages`
    Anthropic,
    /// `POST /v1beta/models/{model}:generateContent`
    Gemini,
    /// QwenWork (千问办公): the request is built and COSY-signed by the
    /// local `qwen` module and never hits the generic OpenAI path.
    #[serde(rename = "qwenwork")]
    QwenWork,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::OpenAiChat => "openai_chat",
            Protocol::OpenAiResponses => "openai_responses",
            Protocol::Anthropic => "anthropic",
            Protocol::Gemini => "gemini",
            Protocol::QwenWork => "qwenwork",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    /// A message carrying the result of a tool call.
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// A model's reasoning. `signature` is the opaque blob vendors use to
    /// validate that a thinking block was not tampered with.
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// An assistant turn asking for a tool to run.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// The user's turn carrying a tool's output.
    ToolResult {
        tool_use_id: String,
        content: Vec<ContentBlock>,
        #[serde(default)]
        is_error: bool,
    },
    /// An inline image (base64 payload).
    Image {
        media_type: String,
        data: String,
    },
    /// A file the user attached, base64 encoded.
    Document {
        media_type: String,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        }
    }

    /// The plain text of a block, for tools that only carry a string.
    pub fn plain_text(&self) -> String {
        match self {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::Thinking { text, .. } => text.clone(),
            ContentBlock::Image { media_type, data } => format!("[image {media_type}:{data}]"),
            ContentBlock::Document {
                media_type,
                data,
                name,
            } => {
                format!(
                    "[document {media_type}:{data}{}]",
                    name.as_deref().unwrap_or("")
                )
            }
            ContentBlock::ToolUse { name, input, .. } => {
                format!(
                    "[tool_use {name}:{}]",
                    serde_json::to_string(input).unwrap_or_default()
                )
            }
            ContentBlock::ToolResult { content, .. } => content
                .iter()
                .map(|b| b.plain_text())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// The tool call this message answers. Only meaningful for `Role::Tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// The name of the tool that produced this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::text(text)],
            tool_call_id: None,
            name: None,
        }
    }

    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Max,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::None => "none",
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::Max => "max",
        }
    }

    /// Anthropic's `thinking.budget_tokens` for this effort level.
    pub fn budget_tokens(&self) -> Option<u32> {
        match self {
            ReasoningEffort::None => Some(0),
            ReasoningEffort::Minimal => Some(1024),
            ReasoningEffort::Low => Some(4096),
            ReasoningEffort::Medium => Some(8192),
            ReasoningEffort::High => Some(16384),
            ReasoningEffort::Max => Some(32768),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    /// Force a specific tool.
    Tool {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema of the tool's parameters.
    #[serde(default)]
    pub input_schema: Value,
}

impl ToolDef {
    pub fn new(name: impl Into<String>, description: Option<String>, input_schema: Value) -> Self {
        ToolDef {
            name: name.into(),
            description,
            input_schema,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// A normalised chat request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatRequest {
    /// The model as the caller asked for it: `provider/model` or `group/id`.
    pub model: String,
    pub system: Vec<ContentBlock>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub tool_choice: Option<ToolChoice>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub stop: Vec<String>,
    pub stream: bool,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Extra headers the caller sent that some vendors care about.
    pub extra_headers: Vec<(String, String)>,
    /// The caller's session id, when it sent one.
    pub session_id: Option<String>,
    /// The working directory the caller ran from, when it reported one.
    /// The key the Usage page groups projects by.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    Error,
}

impl StopReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::StopSequence => "stop_sequence",
            StopReason::ToolUse => "tool_use",
            StopReason::Error => "error",
        }
    }
}

/// A normalised, non-streaming reply.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
}

impl ChatResponse {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> Vec<(&str, &str, &Value)> {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input))
                }
                _ => None,
            })
            .collect()
    }

    /// Build a whole reply out of the events of a stream.
    ///
    /// A vendor that only answers streaming requests still has to serve a
    /// caller that wants one reply; this folds the events back together.
    pub fn from_events(events: &[StreamEvent]) -> ChatResponse {
        let mut id = String::new();
        let mut model = String::new();
        let mut usage = Usage::default();
        let mut stop_reason = None;
        // The text, the reasoning and the tool call of each block index.
        let mut texts: Vec<String> = Vec::new();
        let mut thinkings: Vec<String> = Vec::new();
        let mut tools: Vec<(String, String, String)> = Vec::new();

        for event in events {
            match event {
                StreamEvent::MessageStart {
                    id: start_id,
                    model: start_model,
                    usage: start_usage,
                } => {
                    if id.is_empty() {
                        id = start_id.clone();
                    }
                    if model.is_empty() {
                        model = start_model.clone();
                    }
                    usage = *start_usage;
                }
                StreamEvent::TextDelta { index, text } => {
                    grow(&mut texts, *index);
                    texts[*index].push_str(text);
                }
                StreamEvent::ThinkingDelta { index, text } => {
                    grow(&mut thinkings, *index);
                    thinkings[*index].push_str(text);
                }
                StreamEvent::ToolUseStart {
                    index,
                    id: call_id,
                    name,
                } => {
                    grow(&mut tools, *index);
                    tools[*index].0 = call_id.clone();
                    tools[*index].1 = name.clone();
                }
                StreamEvent::ToolInputDelta {
                    index,
                    partial_json,
                } => {
                    grow(&mut tools, *index);
                    tools[*index].2.push_str(partial_json);
                }
                StreamEvent::MessageDelta {
                    stop_reason: reason,
                    usage: final_usage,
                } => {
                    if reason.is_some() {
                        stop_reason = *reason;
                    }
                    usage = *final_usage;
                }
                _ => {}
            }
        }

        let mut content = Vec::new();
        let blocks = texts.len().max(thinkings.len()).max(tools.len());
        for index in 0..blocks {
            // The reasoning comes before the text it led to.
            if let Some(thinking) = thinkings.get(index) {
                if !thinking.is_empty() {
                    content.push(ContentBlock::Thinking {
                        text: thinking.clone(),
                        signature: None,
                    });
                }
            }
            if let Some(text) = texts.get(index) {
                if !text.is_empty() {
                    content.push(ContentBlock::text(text.clone()));
                }
            }
        }
        for (call_id, name, args) in &tools {
            if name.is_empty() && args.is_empty() {
                continue;
            }
            content.push(ContentBlock::ToolUse {
                id: call_id.clone(),
                name: name.clone(),
                input: serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({})),
            });
        }

        ChatResponse {
            id,
            model,
            content,
            stop_reason,
            usage,
        }
    }
}

/// Make sure a per-index list is long enough to hold `index`.
fn grow<T: Default>(list: &mut Vec<T>, index: usize) {
    while list.len() <= index {
        list.push(T::default());
    }
}

/// A normalised streaming event. The shape follows Anthropic's event
/// vocabulary, which every other protocol can be rendered from.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// The reply has started; carries the id, the model and the first usage.
    MessageStart {
        id: String,
        model: String,
        usage: Usage,
    },
    /// A content block of the given kind has begun.
    ContentBlockStart { index: usize, kind: BlockKind },
    /// More text in the current block.
    TextDelta { index: usize, text: String },
    /// More reasoning in the current block.
    ThinkingDelta { index: usize, text: String },
    /// A tool call has begun.
    ToolUseStart {
        index: usize,
        id: String,
        name: String,
    },
    /// More of a tool call's JSON arguments.
    ToolInputDelta { index: usize, partial_json: String },
    /// A thinking block was signed by the vendor.
    ThinkingSignature { index: usize, signature: String },
    /// The current content block is finished.
    ContentBlockStop { index: usize },
    /// The message's own end: the stop reason and the final usage.
    MessageDelta {
        stop_reason: Option<StopReason>,
        usage: Usage,
    },
    /// The reply is over.
    MessageStop,
    /// The upstream failed mid-stream.
    Error { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Text,
    Thinking,
    ToolUse,
}

/// A failure the caller should see, with the HTTP status it maps to.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    RateLimited(String),
    #[error("{0}")]
    Upstream(String),
    #[error("{0}")]
    Internal(String),
}

impl ApiError {
    pub fn status(&self) -> u16 {
        match self {
            ApiError::BadRequest(_) => 400,
            ApiError::Unauthorized(_) => 401,
            ApiError::NotFound(_) => 404,
            ApiError::RateLimited(_) => 429,
            ApiError::Upstream(_) => 502,
            ApiError::Internal(_) => 500,
        }
    }

    /// Whether the gateway should try the next key or group member.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            ApiError::RateLimited(_) | ApiError::Upstream(_) | ApiError::Internal(_)
        )
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ------------------------------------------------------------ names --

    #[test]
    fn protocol_names_round_trip_through_json() {
        for (value, name) in [
            (Protocol::OpenAiChat, "openai_chat"),
            (Protocol::OpenAiResponses, "openai_responses"),
            (Protocol::Anthropic, "anthropic"),
            (Protocol::Gemini, "gemini"),
            (Protocol::QwenWork, "qwenwork"),
        ] {
            assert_eq!(value.as_str(), name);
            let wire = serde_json::to_value(value).unwrap();
            assert_eq!(wire, json!(name));
            let back: Protocol = serde_json::from_value(wire).unwrap();
            assert_eq!(back, value);
        }
    }

    #[test]
    fn role_and_stop_reason_names_round_trip() {
        for role in [Role::System, Role::User, Role::Assistant, Role::Tool] {
            let back: Role = serde_json::from_value(serde_json::to_value(role).unwrap()).unwrap();
            assert_eq!(back, role);
        }
        for reason in [
            StopReason::EndTurn,
            StopReason::MaxTokens,
            StopReason::StopSequence,
            StopReason::ToolUse,
            StopReason::Error,
        ] {
            let wire = serde_json::to_value(reason).unwrap();
            let back: StopReason = serde_json::from_value(wire).unwrap();
            assert_eq!(back.as_str(), reason.as_str());
        }
    }

    // ----------------------------------------------------------- blocks --

    #[test]
    fn content_blocks_serialize_with_their_tags() {
        let text = ContentBlock::text("hello");
        assert_eq!(
            serde_json::to_value(&text).unwrap(),
            json!({"type": "text", "text": "hello"})
        );
        let use_ = ContentBlock::ToolUse {
            id: "call_1".into(),
            name: "read".into(),
            input: json!({"path": "a.rs"}),
        };
        assert_eq!(
            serde_json::to_value(&use_).unwrap(),
            json!({"type": "tool_use", "id": "call_1", "name": "read", "input": {"path": "a.rs"}})
        );
    }

    #[test]
    fn plain_text_renders_every_block_kind() {
        let blocks = [
            ContentBlock::text("plain"),
            ContentBlock::Thinking {
                text: "hmm".into(),
                signature: None,
            },
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: "QUJD".into(),
            },
            ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: vec![ContentBlock::text("42")],
                is_error: false,
            },
        ];
        let rendered: Vec<String> = blocks.iter().map(|b| b.plain_text()).collect();
        assert_eq!(rendered[0], "plain");
        assert_eq!(rendered[1], "hmm");
        assert!(rendered[2].starts_with("[image image/png:"));
        assert_eq!(rendered[3], "42");
    }

    #[test]
    fn message_helpers_carry_text_only() {
        let user = Message::user("hi");
        assert_eq!(user.role, Role::User);
        assert_eq!(user.text(), "hi");
        assert!(user.tool_call_id.is_none());
        let assistant = Message::assistant("hey");
        assert_eq!(assistant.role, Role::Assistant);
        assert_eq!(assistant.text(), "hey");
    }

    // -------------------------------------------------------- reasoning --

    #[test]
    fn reasoning_effort_maps_to_anthropic_budgets() {
        assert_eq!(ReasoningEffort::None.budget_tokens(), Some(0));
        assert_eq!(ReasoningEffort::Minimal.budget_tokens(), Some(1024));
        assert_eq!(ReasoningEffort::Low.budget_tokens(), Some(4096));
        assert_eq!(ReasoningEffort::Medium.budget_tokens(), Some(8192));
        assert_eq!(ReasoningEffort::High.budget_tokens(), Some(16384));
        assert_eq!(ReasoningEffort::Max.budget_tokens(), Some(32768));
        assert_eq!(ReasoningEffort::High.as_str(), "high");
    }

    // ------------------------------------------------------------ usage --

    #[test]
    fn usage_totals_input_and_output_only() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 100,
            cache_write_tokens: 7,
            reasoning_tokens: 3,
        };
        assert_eq!(usage.total(), 15);
    }

    // -------------------------------------------------------- responses --

    #[test]
    fn response_text_joins_text_blocks_only() {
        let response = ChatResponse {
            id: "r1".into(),
            model: "m".into(),
            content: vec![
                ContentBlock::Thinking {
                    text: "think".into(),
                    signature: None,
                },
                ContentBlock::text("a"),
                ContentBlock::ToolUse {
                    id: "c".into(),
                    name: "f".into(),
                    input: json!({}),
                },
                ContentBlock::text("b"),
            ],
            stop_reason: Some(StopReason::ToolUse),
            usage: Usage::default(),
        };
        assert_eq!(response.text(), "ab");
        assert_eq!(response.tool_uses().len(), 1);
        assert_eq!(response.tool_uses()[0].1, "f");
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
    }

    #[test]
    fn from_events_folds_a_plain_text_stream() {
        let events = vec![
            StreamEvent::MessageStart {
                id: "msg_1".into(),
                model: "glm-5".into(),
                usage: Usage::default(),
            },
            StreamEvent::ContentBlockStart {
                index: 0,
                kind: BlockKind::Text,
            },
            StreamEvent::TextDelta {
                index: 0,
                text: "he".into(),
            },
            StreamEvent::TextDelta {
                index: 0,
                text: "llo".into(),
            },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage {
                    input_tokens: 2,
                    output_tokens: 3,
                    ..Usage::default()
                },
            },
            StreamEvent::MessageStop,
        ];
        let response = ChatResponse::from_events(&events);
        assert_eq!(response.id, "msg_1");
        assert_eq!(response.model, "glm-5");
        assert_eq!(response.text(), "hello");
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.output_tokens, 3);
    }

    #[test]
    fn from_events_keeps_thinking_before_text_and_keeps_it_when_alone() {
        // Reasoning then the text it led to, in one index.
        let paired = ChatResponse::from_events(&[
            StreamEvent::ThinkingDelta {
                index: 0,
                text: "why".into(),
            },
            StreamEvent::TextDelta {
                index: 0,
                text: "42".into(),
            },
        ]);
        assert_eq!(paired.content.len(), 2);
        assert!(matches!(&paired.content[0], ContentBlock::Thinking { text, .. } if text == "why"));
        assert_eq!(paired.content[1].as_text(), Some("42"));

        // Only reasoning arrived: the block must survive, not vanish.
        let alone = ChatResponse::from_events(&[StreamEvent::ThinkingDelta {
            index: 0,
            text: "only".into(),
        }]);
        assert_eq!(alone.content.len(), 1);
        assert!(matches!(&alone.content[0], ContentBlock::Thinking { text, .. } if text == "only"));
    }

    #[test]
    fn from_events_reassembles_tool_json_shards() {
        let response = ChatResponse::from_events(&[
            StreamEvent::ToolUseStart {
                index: 2,
                id: "call_9".into(),
                name: "write".into(),
            },
            StreamEvent::ToolInputDelta {
                index: 2,
                partial_json: "{\"path\":".into(),
            },
            StreamEvent::ToolInputDelta {
                index: 2,
                partial_json: "\"a.rs\"}".into(),
            },
        ]);
        let tools = response.tool_uses();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].0, "call_9");
        assert_eq!(tools[0].1, "write");
        assert_eq!(tools[0].2, &json!({"path": "a.rs"}));
    }

    #[test]
    fn from_events_ignores_an_empty_tool_slot() {
        // A ToolUseStart with no name and no args (some vendors emit one for
        // a cancelled call) must not produce a block.
        let response = ChatResponse::from_events(&[StreamEvent::ToolUseStart {
            index: 0,
            id: String::new(),
            name: String::new(),
        }]);
        assert!(response.content.is_empty());
    }

    #[test]
    fn grow_extends_a_list_to_the_index() {
        let mut list: Vec<u64> = Vec::new();
        grow(&mut list, 2);
        assert_eq!(list, vec![0, 0, 0]);
        grow(&mut list, 1);
        assert_eq!(list, vec![0, 0, 0]);
    }

    // ----------------------------------------------------------- errors --

    #[test]
    fn api_errors_map_to_http_statuses_and_retryability() {
        let cases = [
            (ApiError::BadRequest("bad".into()), 400, false),
            (ApiError::Unauthorized("no".into()), 401, false),
            (ApiError::NotFound("gone".into()), 404, false),
            (ApiError::RateLimited("slow".into()), 429, true),
            (ApiError::Upstream("vendor".into()), 502, true),
            (ApiError::Internal("boom".into()), 500, true),
        ];
        for (error, status, retryable) in cases {
            assert_eq!(error.status(), status);
            assert_eq!(error.retryable(), retryable);
            assert!(!error.to_string().is_empty());
        }
    }
}
