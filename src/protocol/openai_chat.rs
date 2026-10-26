//! OpenAI Chat Completions: `POST /v1/chat/completions`.
//!
//! The format every OpenAI-compatible vendor speaks, so it is also the
//! lingua franca the gateway uses when it talks upstream.

use serde_json::{json, Map, Value};

use super::ir::*;
use super::sse::{SseFrame, DONE};
use crate::gateway::client::StreamCollector;

// ---------------------------------------------------------------- parsing --

pub fn parse_request(v: &Value) -> ApiResult<ChatRequest> {
    let obj = v.as_object().ok_or_else(|| ApiError::BadRequest("the body must be an object".into()))?;
    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("the request has no model".into()))?
        .to_string();

    let mut req = ChatRequest {
        model,
        stream: obj.get("stream").and_then(Value::as_bool).unwrap_or(false),
        ..Default::default()
    };

    req.max_tokens = obj
        .get("max_completion_tokens")
        .or_else(|| obj.get("max_tokens"))
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    req.temperature = obj.get("temperature").and_then(Value::as_f64);
    req.top_p = obj.get("top_p").and_then(Value::as_f64);
    req.top_k = obj.get("top_k").and_then(Value::as_u64).map(|v| v as u32);
    if let Some(stop) = obj.get("stop") {
        match stop {
            Value::String(s) => req.stop.push(s.clone()),
            Value::Array(items) => {
                for i in items {
                    if let Some(s) = i.as_str() {
                        req.stop.push(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    req.reasoning_effort = obj.get("reasoning_effort").and_then(|e| match e.as_str() {
        Some("none") => Some(ReasoningEffort::None),
        Some("minimal") => Some(ReasoningEffort::Minimal),
        Some("low") => Some(ReasoningEffort::Low),
        Some("medium") => Some(ReasoningEffort::Medium),
        Some("high") => Some(ReasoningEffort::High),
        Some("xhigh") | Some("max") => Some(ReasoningEffort::Max),
        _ => None,
    });

    // Tools.
    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for t in tools {
            let f = t.get("function").unwrap_or(t);
            let name = f.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let schema = f
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            req.tools.push(ToolDef {
                name: name.to_string(),
                description: f
                    .get("description")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                input_schema: schema,
            });
        }
    }
    req.tool_choice = match obj.get("tool_choice") {
        Some(Value::String(s)) => match s.as_str() {
            "auto" => Some(ToolChoice::Auto),
            "none" => Some(ToolChoice::None),
            "required" => Some(ToolChoice::Required),
            other => Some(ToolChoice::Tool {
                name: other.to_string(),
            }),
        },
        Some(Value::Object(m)) => {
            let name = m
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .or_else(|| m.get("name").and_then(Value::as_str));
            name.map(|n| ToolChoice::Tool { name: n.to_string() })
        }
        _ => None,
    };

    // Messages.
    let messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::BadRequest("the request has no messages".into()))?;
    for m in messages {
        let role = match m.get("role").and_then(Value::as_str).unwrap_or("user") {
            "system" | "developer" => Role::System,
            "assistant" | "model" => Role::Assistant,
            "tool" | "function" => Role::Tool,
            _ => Role::User,
        };
        let mut blocks = Vec::new();

        match m.get("content") {
            Some(Value::String(s)) => blocks.push(ContentBlock::text(s.clone())),
            Some(Value::Array(parts)) => {
                for p in parts {
                    let kind = p.get("type").and_then(Value::as_str).unwrap_or("text");
                    match kind {
                        "text" => {
                            if let Some(t) = p.get("text").and_then(Value::as_str) {
                                blocks.push(ContentBlock::text(t));
                            }
                        }
                        "image_url" => {
                            if let Some(url) = p
                                .get("image_url")
                                .and_then(|i| i.get("url"))
                                .and_then(Value::as_str)
                            {
                                blocks.push(image_block(url));
                            }
                        }
                        "input_audio" => {
                            // Audio is not something the gateway carries yet.
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        // Tool calls live on the assistant message.
        if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
            for c in calls {
                let id = c
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(c["function"]["name"].as_str().unwrap_or_default())
                    .to_string();
                let name = c
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let args = c
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                blocks.push(ContentBlock::ToolUse {
                    id,
                    name,
                    input: parse_json_loose(args),
                });
            }
        }
        // The older `function_call` shape.
        if let Some(fc) = m.get("function_call") {
            let name = fc
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let args = fc.get("arguments").and_then(Value::as_str).unwrap_or("{}");
            blocks.push(ContentBlock::ToolUse {
                id: format!("call_{name}"),
                name,
                input: parse_json_loose(args),
            });
        }

        if role == Role::System {
            req.system.extend(blocks);
            continue;
        }

        // A tool result message keeps its own id and name.
        if role == Role::Tool {
            let tool_use_id = m
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = m.get("name").and_then(Value::as_str).map(|s| s.to_string());
            let content = blocks
                .into_iter()
                .map(|b| ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: vec![b],
                    is_error: false,
                })
                .collect();
            req.messages.push(Message {
                role: Role::Tool,
                content,
                tool_call_id: Some(tool_use_id),
                name,
            });
            continue;
        }

        req.messages.push(Message {
            role,
            content: blocks,
            tool_call_id: None,
            name: None,
        });
    }

    Ok(req)
}

/// Turn an OpenAI `image_url` part into a block, splitting a data url.
pub fn image_block(url: &str) -> ContentBlock {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((mime, b64)) = rest.split_once(";base64,") {
            return ContentBlock::Image {
                media_type: mime.to_string(),
                data: b64.to_string(),
            };
        }
    }
    ContentBlock::Image {
        media_type: "image/url".to_string(),
        data: url.to_string(),
    }
}

/// Parse JSON that may have been truncated or wrapped in a fence.
pub fn parse_json_loose(s: &str) -> Value {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v;
    }
    // Cut at the last complete brace.
    if let Some(idx) = trimmed.rfind('}') {
        if let Ok(v) = serde_json::from_str::<Value>(&trimmed[..=idx]) {
            return v;
        }
    }
    json!({ "_raw": s })
}

pub fn parse_response(v: &Value) -> ApiResult<ChatResponse> {
    let obj = v.as_object().ok_or_else(|| ApiError::Upstream("the reply was not an object".into()))?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-unknown")
        .to_string();
    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut content = Vec::new();
    let mut stop_reason = None;

    if let Some(choice) = obj
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
    {
        let msg = choice.get("message").unwrap_or(&Value::Null);
        if let Some(t) = msg.get("reasoning_content").and_then(Value::as_str) {
            if !t.is_empty() {
                content.push(ContentBlock::Thinking {
                    text: t.to_string(),
                    signature: None,
                });
            }
        }
        if let Some(t) = msg.get("reasoning").and_then(|r| r.get("content")).and_then(Value::as_str) {
            if !t.is_empty() {
                content.push(ContentBlock::Thinking {
                    text: t.to_string(),
                    signature: None,
                });
            }
        }
        match msg.get("content") {
            Some(Value::String(s)) if !s.is_empty() => content.push(ContentBlock::text(s.clone())),
            Some(Value::Array(parts)) => {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        content.push(ContentBlock::text(t));
                    }
                }
            }
            _ => {}
        }
        if let Some(calls) = msg.get("tool_calls").and_then(Value::as_array) {
            for c in calls {
                let id = c
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = c
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let args = c
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                content.push(ContentBlock::ToolUse {
                    id,
                    name,
                    input: parse_json_loose(args),
                });
            }
        }
        stop_reason = match choice.get("finish_reason").and_then(Value::as_str) {
            Some("stop") => Some(StopReason::EndTurn),
            Some("length") => Some(StopReason::MaxTokens),
            Some("tool_calls") | Some("function_call") => Some(StopReason::ToolUse),
            _ => None,
        };
    }

    let usage = parse_usage(obj.get("usage"));
    Ok(ChatResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

pub fn parse_usage(u: Option<&Value>) -> Usage {
    let mut usage = Usage::default();
    let Some(u) = u else { return usage };
    usage.input_tokens = u
        .get("prompt_tokens")
        .or_else(|| u.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.output_tokens = u
        .get("completion_tokens")
        .or_else(|| u.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_read_tokens = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .or_else(|| u.get("cache_read_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_write_tokens = u
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.reasoning_tokens = u
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage
}

// -------------------------------------------------------------- formatting --

/// Write a canonical request in the OpenAI Chat shape, for an upstream.
pub fn format_request(req: &ChatRequest, model: &str) -> Value {
    let mut messages = Vec::new();

    for block in &req.system {
        if let ContentBlock::Text { text } = block {
            messages.push(json!({"role": "system", "content": text}));
        }
    }

    for m in &req.messages {
        match m.role {
            Role::System => {
                for b in &m.content {
                    if let ContentBlock::Text { text } = b {
                        messages.push(json!({"role": "system", "content": text}));
                    }
                }
            }
            Role::User => {
                let parts = user_parts(&m.content);
                messages.push(json!({"role": "user", "content": parts}));
            }
            Role::Assistant => {
                let mut text = String::new();
                let mut reasoning = String::new();
                let mut tool_calls = Vec::new();
                for b in &m.content {
                    match b {
                        ContentBlock::Text { text: t } => text.push_str(t),
                        ContentBlock::Thinking { text: t, .. } => reasoning.push_str(t),
                        ContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                            }
                        })),
                        _ => {}
                    }
                }
                let mut msg = Map::new();
                msg.insert("role".into(), json!("assistant"));
                if !reasoning.is_empty() {
                    msg.insert("reasoning_content".into(), json!(reasoning));
                }
                if text.is_empty() && tool_calls.is_empty() {
                    msg.insert("content".into(), json!(""));
                } else {
                    msg.insert("content".into(), json!(text));
                }
                if !tool_calls.is_empty() {
                    msg.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                messages.push(Value::Object(msg));
            }
            Role::Tool => {
                for b in &m.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = b
                    {
                        let text = content
                            .iter()
                            .map(|c| c.plain_text())
                            .collect::<Vec<_>>()
                            .join("\n");
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": text,
                        }));
                    }
                }
            }
        }
    }

    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("messages".into(), Value::Array(messages));

    if let Some(n) = req.max_tokens {
        body.insert("max_tokens".into(), json!(n));
    }
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        body.insert("top_p".into(), json!(p));
    }
    if !req.stop.is_empty() {
        body.insert("stop".into(), json!(req.stop));
    }
    if let Some(effort) = req.reasoning_effort {
        body.insert("reasoning_effort".into(), json!(effort.as_str()));
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description.clone().unwrap_or_default(),
                        "parameters": t.input_schema.clone(),
                    }
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }
    if let Some(choice) = &req.tool_choice {
        body.insert(
            "tool_choice".into(),
            match choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
                ToolChoice::Required => json!("required"),
                ToolChoice::Tool { name } => {
                    json!({"type": "function", "function": {"name": name}})
                }
            },
        );
    }
    if req.stream {
        body.insert("stream".into(), json!(true));
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    Value::Object(body)
}

fn user_parts(content: &[ContentBlock]) -> Value {
    let mut parts = Vec::new();
    for b in content {
        match b {
            ContentBlock::Text { text } => {
                if !text.is_empty() {
                    parts.push(json!({"type": "text", "text": text}));
                }
            }
            ContentBlock::Image { media_type, data } => {
                if media_type == "image/url" {
                    parts.push(json!({"type": "image_url", "image_url": {"url": data}}));
                } else {
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": {"url": format!("data:{media_type};base64,{data}")},
                    }));
                }
            }
            ContentBlock::Document {
                media_type,
                data,
                name,
            } => {
                parts.push(json!({
                    "type": "text",
                    "text": format!(
                        "[file {}{}]",
                        name.clone().unwrap_or_default(),
                        if data.is_empty() { String::new() } else { format!(" ({media_type})") }
                    ),
                }));
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        parts.push(json!({"type": "text", "text": ""}));
    }
    Value::Array(parts)
}

/// Write a canonical reply in the OpenAI Chat shape, for a caller.
pub fn format_response(resp: &ChatResponse, model: &str) -> Value {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();

    for b in &resp.content {
        match b {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::Thinking { text: t, .. } => reasoning.push_str(t),
            ContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                }
            })),
            _ => {}
        }
    }

    let mut message = Map::new();
    message.insert("role".into(), json!("assistant"));
    if !reasoning.is_empty() {
        message.insert("reasoning_content".into(), json!(reasoning));
    }
    message.insert("content".into(), json!(text));
    if !tool_calls.is_empty() {
        message.insert("tool_calls".into(), Value::Array(tool_calls));
    }

    let finish = match resp.stop_reason {
        Some(StopReason::MaxTokens) => "length",
        Some(StopReason::ToolUse) => "tool_calls",
        Some(StopReason::StopSequence) => "stop",
        _ => "stop",
    };

    json!({
        "id": resp.id,
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "logprobs": Value::Null,
            "finish_reason": finish,
        }],
        "usage": {
            "prompt_tokens": resp.usage.input_tokens,
            "completion_tokens": resp.usage.output_tokens,
            "total_tokens": resp.usage.total(),
            "prompt_tokens_details": {"cached_tokens": resp.usage.cache_read_tokens},
            "completion_tokens_details": {"reasoning_tokens": resp.usage.reasoning_tokens},
        }
    })
}

// ---------------------------------------------------------------- streams --

/// Reads an OpenAI Chat stream into canonical events.
pub struct Collector {
    pub id: String,
    pub model: String,
    pub created: i64,
    pub started: bool,
    pub finished: bool,
    pub content_index: usize,
    pub text_index: Option<usize>,
    pub reasoning_index: Option<usize>,
    pub tool_indices: Vec<(usize, usize)>,
    pub usage: Usage,
    pub stop_reason: Option<StopReason>,
}

impl Collector {
    pub fn new() -> Self {
        Collector {
            id: "chatcmpl-unknown".to_string(),
            model: String::new(),
            created: chrono::Utc::now().timestamp(),
            started: false,
            finished: false,
            content_index: 0,
            text_index: None,
            reasoning_index: None,
            tool_indices: Vec::new(),
            usage: Usage::default(),
            stop_reason: None,
        }
    }

    /// Open a block of the given kind, closing the one before it.
    fn open(&mut self, kind: BlockKind, out: &mut Vec<StreamEvent>) -> usize {
        if let Some(index) = self.text_index.take() {
            out.push(StreamEvent::ContentBlockStop { index });
        }
        let index = self.content_index;
        self.content_index += 1;
        out.push(StreamEvent::ContentBlockStart { index, kind });
        index
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamCollector for Collector {
    fn push(&mut self, chunk: &Value, out: &mut Vec<StreamEvent>) {
        if let Some(id) = chunk.get("id").and_then(Value::as_str) {
            self.id = id.to_string();
        }
        if let Some(model) = chunk.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        if let Some(usage) = chunk.get("usage") {
            if !usage.is_null() {
                let u = parse_usage(Some(usage));
                self.usage.input_tokens = u.input_tokens.max(self.usage.input_tokens);
                self.usage.output_tokens = u.output_tokens.max(self.usage.output_tokens);
                self.usage.cache_read_tokens = u.cache_read_tokens.max(self.usage.cache_read_tokens);
                self.usage.cache_write_tokens = u.cache_write_tokens.max(self.usage.cache_write_tokens);
                self.usage.reasoning_tokens = u.reasoning_tokens.max(self.usage.reasoning_tokens);
            }
        }
        if !self.started {
            self.started = true;
            out.push(StreamEvent::MessageStart {
                id: self.id.clone(),
                model: self.model.clone(),
                usage: self.usage,
            });
        }

        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return;
        };
        let delta = choice.get("delta").unwrap_or(&Value::Null);

        if let Some(t) = delta.get("reasoning_content").and_then(Value::as_str) {
            if !t.is_empty() {
                let index = match self.reasoning_index {
                    Some(i) => i,
                    None => {
                        let i = self.open(BlockKind::Thinking, out);
                        self.reasoning_index = Some(i);
                        i
                    }
                };
                out.push(StreamEvent::ThinkingDelta {
                    index,
                    text: t.to_string(),
                });
            }
        }

        match delta.get("content") {
            Some(Value::String(s)) if !s.is_empty() => {
                let index = match self.text_index {
                    Some(i) => i,
                    None => {
                        let i = self.open(BlockKind::Text, out);
                        self.text_index = Some(i);
                        i
                    }
                };
                out.push(StreamEvent::TextDelta {
                    index,
                    text: s.clone(),
                });
            }
            Some(Value::Array(parts)) => {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        let index = match self.text_index {
                            Some(i) => i,
                            None => {
                                let i = self.open(BlockKind::Text, out);
                                self.text_index = Some(i);
                                i
                            }
                        };
                        out.push(StreamEvent::TextDelta {
                            index,
                            text: t.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }

        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for c in calls {
                let slot = c.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let existing = self.tool_indices.iter().find(|(s, _)| *s == slot).map(|(_, i)| *i);
                let index = match existing {
                    Some(i) => i,
                    None => {
                        let i = self.open(BlockKind::ToolUse, out);
                        self.tool_indices.push((slot, i));
                        i
                    }
                };
                let id = c.get("id").and_then(Value::as_str).unwrap_or_default();
                let name = c
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if existing.is_none() {
                    out.push(StreamEvent::ContentBlockStart {
                        index,
                        kind: BlockKind::ToolUse,
                    });
                    out.push(StreamEvent::ToolUseStart {
                        index,
                        id: id.to_string(),
                        name: name.to_string(),
                    });
                }
                if let Some(args) = c
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                {
                    if !args.is_empty() {
                        out.push(StreamEvent::ToolInputDelta {
                            index,
                            partial_json: args.to_string(),
                        });
                    }
                }
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = match reason {
                "length" => Some(StopReason::MaxTokens),
                "tool_calls" | "function_call" => Some(StopReason::ToolUse),
                "stop" => Some(StopReason::EndTurn),
                _ => None,
            };
        }
    }

    fn finish(&mut self, out: &mut Vec<StreamEvent>) {
        if self.finished {
            return;
        }
        self.finished = true;
        if !self.started {
            self.started = true;
            out.push(StreamEvent::MessageStart {
                id: self.id.clone(),
                model: self.model.clone(),
                usage: self.usage,
            });
        }
        for (slot, index) in self.tool_indices.clone() {
            let _ = slot;
            out.push(StreamEvent::ContentBlockStop { index });
        }
        if let Some(i) = self.text_index.take() {
            out.push(StreamEvent::ContentBlockStop { index: i });
        }
        if let Some(i) = self.reasoning_index {
            out.push(StreamEvent::ContentBlockStop { index: i });
        }
        out.push(StreamEvent::MessageDelta {
            stop_reason: self.stop_reason,
            usage: self.usage,
        });
        out.push(StreamEvent::MessageStop);
    }
}

/// Renders canonical events back into an OpenAI Chat stream.
pub struct Renderer {
    pub id: String,
    pub model: String,
    pub created: i64,
    pub role_sent: bool,
    pub tool_slots: Vec<(usize, usize)>,
    pub usage: Usage,
    pub finish_sent: bool,
}

impl Renderer {
    pub fn new(id: String, model: String) -> Self {
        Renderer {
            id,
            model,
            created: chrono::Utc::now().timestamp(),
            role_sent: false,
            tool_slots: Vec::new(),
            usage: Usage::default(),
            finish_sent: false,
        }
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> SseFrame {
        SseFrame::data(
            json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [{"index": 0, "delta": delta, "logprobs": Value::Null, "finish_reason": finish}],
            })
            .to_string(),
        )
    }

    pub fn render(&mut self, ev: &StreamEvent) -> Vec<SseFrame> {
        let mut frames = Vec::new();
        match ev {
            StreamEvent::MessageStart { id, model, usage } => {
                self.id = id.clone();
                if self.model.is_empty() && !model.is_empty() {
                    self.model = model.clone();
                }
                self.usage = *usage;
                let mut delta = Map::new();
                delta.insert("role".into(), json!("assistant"));
                delta.insert("content".into(), json!(""));
                self.role_sent = true;
                frames.push(self.chunk(Value::Object(delta), None));
            }
            StreamEvent::ContentBlockStart { .. } => {}
            StreamEvent::TextDelta { text, .. } => {
                frames.push(self.chunk(json!({"content": text}), None));
            }
            StreamEvent::ThinkingDelta { text, .. } => {
                frames.push(self.chunk(json!({"reasoning_content": text}), None));
            }
            StreamEvent::ToolUseStart { index, id, name } => {
                let slot = self.tool_slots.len();
                self.tool_slots.push((*index, slot));
                frames.push(self.chunk(
                    json!({
                        "tool_calls": [{
                            "index": slot,
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": ""},
                        }]
                    }),
                    None,
                ));
            }
            StreamEvent::ToolInputDelta { index, partial_json } => {
                let slot = self
                    .tool_slots
                    .iter()
                    .find(|(i, _)| i == index)
                    .map(|(_, s)| *s)
                    .unwrap_or(0);
                frames.push(self.chunk(
                    json!({
                        "tool_calls": [{
                            "index": slot,
                            "function": {"arguments": partial_json},
                        }]
                    }),
                    None,
                ));
            }
            StreamEvent::ContentBlockStop { .. } => {}
            StreamEvent::ThinkingSignature { .. } => {}
            StreamEvent::MessageDelta { stop_reason, usage } => {
                self.usage = *usage;
                let finish = match stop_reason {
                    Some(StopReason::MaxTokens) => "length",
                    Some(StopReason::ToolUse) => "tool_calls",
                    _ => "stop",
                };
                if !self.finish_sent {
                    self.finish_sent = true;
                    frames.push(self.chunk(json!({}), Some(finish)));
                }
            }
            StreamEvent::MessageStop => {
                if self.usage.input_tokens > 0 || self.usage.output_tokens > 0 {
                    frames.push(SseFrame::data(
                        json!({
                            "id": self.id,
                            "object": "chat.completion.chunk",
                            "created": self.created,
                            "model": self.model,
                            "choices": [],
                            "usage": {
                                "prompt_tokens": self.usage.input_tokens,
                                "completion_tokens": self.usage.output_tokens,
                                "total_tokens": self.usage.total(),
                                "prompt_tokens_details": {"cached_tokens": self.usage.cache_read_tokens},
                                "completion_tokens_details": {"reasoning_tokens": self.usage.reasoning_tokens},
                            }
                        })
                        .to_string(),
                    ));
                }
                frames.push(SseFrame::data(DONE));
            }
            StreamEvent::Error { message } => {
                frames.push(SseFrame::data(
                    json!({"error": {"message": message, "type": "upstream_error"}}).to_string(),
                ));
                frames.push(SseFrame::data(DONE));
            }
        }
        frames
    }
}
