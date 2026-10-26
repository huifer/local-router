//! OpenAI Responses: `POST /v1/responses`.
//!
//! The newer OpenAI API, spoken by Codex and by a growing set of agents.

use serde_json::{json, Map, Value};

use super::ir::*;
use super::sse::SseFrame;
use crate::gateway::client::StreamCollector;

// ---------------------------------------------------------------- parsing --

pub fn parse_request(v: &Value) -> ApiResult<ChatRequest> {
    let obj = v
        .as_object()
        .ok_or_else(|| ApiError::BadRequest("the body must be an object".into()))?;
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
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    req.temperature = obj.get("temperature").and_then(Value::as_f64);
    req.top_p = obj.get("top_p").and_then(Value::as_f64);
    if let Some(effort) = obj
        .get("reasoning")
        .and_then(|r| r.get("effort"))
        .and_then(Value::as_str)
    {
        req.reasoning_effort = match effort {
            "none" => Some(ReasoningEffort::None),
            "minimal" => Some(ReasoningEffort::Minimal),
            "low" => Some(ReasoningEffort::Low),
            "medium" => Some(ReasoningEffort::Medium),
            "high" => Some(ReasoningEffort::High),
            "xhigh" => Some(ReasoningEffort::Max),
            _ => None,
        };
    }

    if let Some(instructions) = obj.get("instructions").and_then(Value::as_str) {
        if !instructions.is_empty() {
            req.system.push(ContentBlock::text(instructions));
        }
    }

    // Tools.
    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for t in tools {
            match t.get("type").and_then(Value::as_str) {
                Some("function") | None => {
                    let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    req.tools.push(ToolDef {
                        name: name.to_string(),
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string()),
                        input_schema: t
                            .get("parameters")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    });
                }
                Some("web_search_preview") | Some("web_search") => {}
                _ => {}
            }
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
        Some(Value::Object(m)) => m
            .get("name")
            .and_then(Value::as_str)
            .map(|n| ToolChoice::Tool {
                name: n.to_string(),
            }),
        _ => None,
    };

    // Input.
    match obj.get("input") {
        Some(Value::String(s)) => req.messages.push(Message::user(s.clone())),
        Some(Value::Array(items)) => {
            for item in items {
                let kind = item.get("type").and_then(Value::as_str);
                match kind {
                    Some("function_call") => {
                        req.messages.push(Message {
                            role: Role::Assistant,
                            content: vec![ContentBlock::ToolUse {
                                id: item
                                    .get("call_id")
                                    .and_then(Value::as_str)
                                    .or_else(|| item.get("id").and_then(Value::as_str))
                                    .unwrap_or_default()
                                    .to_string(),
                                name: item
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                input: super::openai_chat::parse_json_loose(
                                    item.get("arguments")
                                        .and_then(Value::as_str)
                                        .unwrap_or("{}"),
                                ),
                            }],
                            tool_call_id: None,
                            name: None,
                        });
                    }
                    Some("function_call_output") => {
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let output = item.get("output").cloned().unwrap_or(Value::Null);
                        let text = match output {
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        req.messages.push(Message {
                            role: Role::Tool,
                            content: vec![ContentBlock::ToolResult {
                                tool_use_id: call_id.clone(),
                                content: vec![ContentBlock::text(text)],
                                is_error: false,
                            }],
                            tool_call_id: Some(call_id),
                            name: None,
                        });
                    }
                    Some("reasoning") => {
                        let text = item
                            .get("summary")
                            .and_then(Value::as_array)
                            .map(|parts| {
                                parts
                                    .iter()
                                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                                    .collect::<Vec<_>>()
                                    .join("")
                            })
                            .unwrap_or_default();
                        if !text.is_empty() {
                            req.messages.push(Message {
                                role: Role::Assistant,
                                content: vec![ContentBlock::Thinking {
                                    text,
                                    signature: None,
                                }],
                                tool_call_id: None,
                                name: None,
                            });
                        }
                    }
                    _ => {
                        let role = match item.get("role").and_then(Value::as_str) {
                            Some("assistant") => Role::Assistant,
                            Some("system") | Some("developer") => Role::System,
                            _ => Role::User,
                        };
                        let mut blocks = Vec::new();
                        match item.get("content") {
                            Some(Value::String(s)) => blocks.push(ContentBlock::text(s.clone())),
                            Some(Value::Array(parts)) => {
                                for p in parts {
                                    let ptype = p
                                        .get("type")
                                        .and_then(Value::as_str)
                                        .unwrap_or("input_text");
                                    match ptype {
                                        "input_text" | "output_text" | "text" | "summary_text" => {
                                            if let Some(t) = p.get("text").and_then(Value::as_str) {
                                                blocks.push(ContentBlock::text(t));
                                            }
                                        }
                                        "input_image" => {
                                            let url = p
                                                .get("image_url")
                                                .and_then(Value::as_str)
                                                .unwrap_or_default();
                                            blocks.push(super::openai_chat::image_block(url));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                        if role == Role::System {
                            req.system.extend(blocks);
                        } else {
                            req.messages.push(Message {
                                role,
                                content: blocks,
                                tool_call_id: None,
                                name: None,
                            });
                        }
                    }
                }
            }
        }
        _ => {}
    }

    Ok(req)
}

pub fn parse_response(v: &Value) -> ApiResult<ChatResponse> {
    let obj = v
        .as_object()
        .ok_or_else(|| ApiError::Upstream("the reply was not an object".into()))?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_unknown")
        .to_string();
    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut content = Vec::new();
    let mut stop_reason = None;

    if let Some(output) = obj.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for p in parts {
                            if let Some(t) = p.get("text").and_then(Value::as_str) {
                                content.push(ContentBlock::text(t));
                            }
                        }
                    }
                }
                Some("reasoning") => {
                    let text = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|p| p.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default();
                    if !text.is_empty() {
                        content.push(ContentBlock::Thinking {
                            text,
                            signature: None,
                        });
                    }
                }
                Some("function_call") => {
                    content.push(ContentBlock::ToolUse {
                        id: item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("id").and_then(Value::as_str))
                            .unwrap_or_default()
                            .to_string(),
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input: super::openai_chat::parse_json_loose(
                            item.get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}"),
                        ),
                    });
                }
                _ => {}
            }
        }
    }

    if !content.is_empty() {
        let has_tool = content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        stop_reason = Some(if has_tool {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        });
    }
    if let Some(status) = obj.get("status").and_then(Value::as_str) {
        if status == "incomplete" {
            stop_reason = Some(StopReason::MaxTokens);
        }
    }

    let u = obj.get("usage");
    let usage = Usage {
        input_tokens: u
            .and_then(|u| u.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: u
            .and_then(|u| u.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens: u
            .and_then(|u| u.get("input_tokens_details"))
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_write_tokens: 0,
        reasoning_tokens: u
            .and_then(|u| u.get("output_tokens_details"))
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    };

    Ok(ChatResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

// -------------------------------------------------------------- formatting --

pub fn format_request(req: &ChatRequest, model: &str) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(model));

    let system_text = req
        .system
        .iter()
        .map(|b| b.plain_text())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system_text.is_empty() {
        body.insert("instructions".into(), json!(system_text));
    }

    let mut input = Vec::new();
    for m in &req.messages {
        match m.role {
            Role::System => {
                let text = m.content.iter().map(|b| b.plain_text()).collect::<String>();
                if !text.is_empty() {
                    input.push(json!({
                        "role": "system",
                        "content": [{"type": "input_text", "text": text}],
                    }));
                }
            }
            Role::User => {
                let parts: Vec<Value> = m
                    .content
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => {
                            json!({"type": "input_text", "text": text})
                        }
                        ContentBlock::Image { media_type, data } => {
                            if media_type == "image/url" {
                                json!({"type": "input_image", "image_url": data})
                            } else {
                                json!({
                                    "type": "input_image",
                                    "image_url": format!("data:{media_type};base64,{data}"),
                                })
                            }
                        }
                        _ => json!({"type": "input_text", "text": b.plain_text()}),
                    })
                    .collect();
                input.push(json!({"role": "user", "content": parts}));
            }
            Role::Assistant => {
                for b in &m.content {
                    match b {
                        ContentBlock::Text { text } => {
                            input.push(json!({
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": text}],
                            }));
                        }
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input: args,
                        } => {
                            input.push(json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": serde_json::to_string(args).unwrap_or_else(|_| "{}".into()),
                            }));
                        }
                        _ => {}
                    }
                }
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
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_use_id,
                            "output": text,
                        }));
                    }
                }
            }
        }
    }
    body.insert("input".into(), Value::Array(input));

    if let Some(n) = req.max_tokens {
        body.insert("max_output_tokens".into(), json!(n));
    }
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        body.insert("top_p".into(), json!(p));
    }
    if let Some(effort) = req.reasoning_effort {
        body.insert(
            "reasoning".into(),
            json!({"effort": effort.as_str(), "summary": "auto"}),
        );
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description.clone().unwrap_or_default(),
                    "parameters": t.input_schema.clone(),
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
                ToolChoice::Tool { name } => json!({"type": "function", "name": name}),
            },
        );
    }
    if req.stream {
        body.insert("stream".into(), json!(true));
    }
    Value::Object(body)
}

pub fn format_response(resp: &ChatResponse, model: &str) -> Value {
    let mut output = Vec::new();
    let mut text = String::new();
    let mut reasoning = String::new();

    for b in &resp.content {
        match b {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::Thinking { text: t, .. } => reasoning.push_str(t),
            _ => {}
        }
    }
    if !reasoning.is_empty() {
        output.push(json!({
            "type": "reasoning",
            "id": format!("rs_{}", resp.id),
            "summary": [{"type": "summary_text", "text": reasoning}],
        }));
    }
    if !text.is_empty() {
        output.push(json!({
            "type": "message",
            "id": format!("msg_{}", resp.id),
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": text, "annotations": []}],
        }));
    }
    for b in &resp.content {
        if let ContentBlock::ToolUse { id, name, input } = b {
            output.push(json!({
                "type": "function_call",
                "id": format!("fc_{id}"),
                "call_id": id,
                "name": name,
                "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                "status": "completed",
            }));
        }
    }

    json!({
        "id": resp.id,
        "object": "response",
        "created_at": chrono::Utc::now().timestamp(),
        "status": "completed",
        "model": model,
        "output": output,
        "output_text": resp.text(),
        "usage": {
            "input_tokens": resp.usage.input_tokens,
            "output_tokens": resp.usage.output_tokens,
            "total_tokens": resp.usage.total(),
            "input_tokens_details": {"cached_tokens": resp.usage.cache_read_tokens},
            "output_tokens_details": {"reasoning_tokens": resp.usage.reasoning_tokens},
        }
    })
}

// ---------------------------------------------------------------- streams --

pub struct Collector {
    pub id: String,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: Option<StopReason>,
    pub started: bool,
    pub finished: bool,
    pub next_index: usize,
    pub open_block: Option<(usize, BlockKind)>,
    pub tool_items: Vec<(usize, String)>,
}

impl Collector {
    pub fn new() -> Self {
        Collector {
            id: "resp_unknown".to_string(),
            model: String::new(),
            usage: Usage::default(),
            stop_reason: None,
            started: false,
            finished: false,
            next_index: 0,
            open_block: None,
            tool_items: Vec::new(),
        }
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamCollector for Collector {
    fn push(&mut self, chunk: &Value, out: &mut Vec<StreamEvent>) {
        let kind = chunk.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "response.created" | "response.in_progress" => {
                let resp = chunk.get("response").cloned().unwrap_or(Value::Null);
                if let Some(id) = resp.get("id").and_then(Value::as_str) {
                    self.id = id.to_string();
                }
                if let Some(model) = resp.get("model").and_then(Value::as_str) {
                    self.model = model.to_string();
                }
                if !self.started {
                    self.started = true;
                    out.push(StreamEvent::MessageStart {
                        id: self.id.clone(),
                        model: self.model.clone(),
                        usage: self.usage,
                    });
                }
            }
            "response.output_item.added" => {
                let item = chunk.get("item").cloned().unwrap_or(Value::Null);
                let itype = item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message");
                let index = self.next_index;
                self.next_index += 1;
                match itype {
                    "reasoning" => {
                        out.push(StreamEvent::ContentBlockStart {
                            index,
                            kind: BlockKind::Thinking,
                        });
                        self.open_block = Some((index, BlockKind::Thinking));
                    }
                    "function_call" => {
                        out.push(StreamEvent::ContentBlockStart {
                            index,
                            kind: BlockKind::ToolUse,
                        });
                        out.push(StreamEvent::ToolUseStart {
                            index,
                            id: item
                                .get("call_id")
                                .and_then(Value::as_str)
                                .or_else(|| item.get("id").and_then(Value::as_str))
                                .unwrap_or_default()
                                .to_string(),
                            name: item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        });
                        self.open_block = Some((index, BlockKind::ToolUse));
                    }
                    _ => {
                        out.push(StreamEvent::ContentBlockStart {
                            index,
                            kind: BlockKind::Text,
                        });
                        self.open_block = Some((index, BlockKind::Text));
                    }
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let index = self
                    .open_block
                    .filter(|(_, k)| *k == BlockKind::Text)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                if let Some(t) = chunk.get("delta").and_then(Value::as_str) {
                    out.push(StreamEvent::TextDelta {
                        index,
                        text: t.to_string(),
                    });
                }
            }
            "response.reasoning_summary_text.delta" => {
                let index = self
                    .open_block
                    .filter(|(_, k)| *k == BlockKind::Thinking)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                if let Some(t) = chunk.get("delta").and_then(Value::as_str) {
                    out.push(StreamEvent::ThinkingDelta {
                        index,
                        text: t.to_string(),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                let index = self
                    .open_block
                    .filter(|(_, k)| *k == BlockKind::ToolUse)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                if let Some(t) = chunk.get("delta").and_then(Value::as_str) {
                    out.push(StreamEvent::ToolInputDelta {
                        index,
                        partial_json: t.to_string(),
                    });
                }
            }
            "response.output_item.done" => {
                if let Some((index, _)) = self.open_block.take() {
                    out.push(StreamEvent::ContentBlockStop { index });
                }
            }
            "response.completed" | "response.incomplete" => {
                let resp = chunk.get("response").cloned().unwrap_or(Value::Null);
                if let Some(id) = resp.get("id").and_then(Value::as_str) {
                    self.id = id.to_string();
                }
                if let Some(u) = resp.get("usage") {
                    let usage = Usage {
                        input_tokens: u
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(self.usage.input_tokens),
                        output_tokens: u
                            .get("output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(self.usage.output_tokens),
                        cache_read_tokens: u
                            .get("input_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(Value::as_u64)
                            .unwrap_or(self.usage.cache_read_tokens),
                        cache_write_tokens: 0,
                        reasoning_tokens: u
                            .get("output_tokens_details")
                            .and_then(|d| d.get("reasoning_tokens"))
                            .and_then(Value::as_u64)
                            .unwrap_or(self.usage.reasoning_tokens),
                    };
                    self.usage = usage;
                }
                if let Some(output) = resp.get("output").and_then(Value::as_array) {
                    if output
                        .iter()
                        .any(|i| i.get("type").and_then(Value::as_str) == Some("function_call"))
                    {
                        self.stop_reason = Some(StopReason::ToolUse);
                    }
                }
                if chunk.get("type").and_then(Value::as_str) == Some("response.incomplete") {
                    self.stop_reason = Some(StopReason::MaxTokens);
                }
            }
            "response.failed" | "error" => {
                let message = chunk
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .or_else(|| chunk.get("message").and_then(Value::as_str))
                    .unwrap_or("the upstream failed")
                    .to_string();
                out.push(StreamEvent::Error { message });
            }
            _ => {}
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
        if let Some((index, _)) = self.open_block.take() {
            out.push(StreamEvent::ContentBlockStop { index });
        }
        out.push(StreamEvent::MessageDelta {
            stop_reason: self.stop_reason,
            usage: self.usage,
        });
        out.push(StreamEvent::MessageStop);
    }
}

/// Renders canonical events back into an OpenAI Responses stream.
pub struct Renderer {
    pub id: String,
    pub model: String,
    pub sequence: u64,
    pub started: bool,
    pub output_index: usize,
    pub item_index: usize,
    pub text_open: bool,
    pub usage: Usage,
    pub text: String,
    pub tool_items: Vec<(usize, String, String)>,
    pub stopped: bool,
}

impl Renderer {
    pub fn new(id: String, model: String) -> Self {
        Renderer {
            id,
            model,
            sequence: 0,
            started: false,
            output_index: 0,
            item_index: 0,
            text_open: false,
            usage: Usage::default(),
            text: String::new(),
            tool_items: Vec::new(),
            stopped: false,
        }
    }

    fn event(&mut self, kind: &str, body: Value) -> SseFrame {
        self.sequence += 1;
        let mut merged = Map::new();
        merged.insert("type".into(), json!(kind));
        merged.insert("sequence_number".into(), json!(self.sequence));
        if let Some(obj) = body.as_object() {
            for (k, v) in obj {
                merged.insert(k.clone(), v.clone());
            }
        }
        SseFrame::data(Value::Object(merged).to_string())
    }

    pub fn render(&mut self, ev: &StreamEvent) -> Vec<SseFrame> {
        let mut frames = Vec::new();
        match ev {
            StreamEvent::MessageStart { id, model, usage } => {
                if self.started {
                    return frames;
                }
                self.started = true;
                self.id = id.clone();
                if self.model.is_empty() && !model.is_empty() {
                    self.model = model.clone();
                }
                self.usage = *usage;
                frames.push(self.event(
                    "response.created",
                    json!({"response": {
                        "id": self.id,
                        "object": "response",
                        "status": "in_progress",
                        "model": self.model,
                        "output": [],
                    }}),
                ));
            }
            StreamEvent::ContentBlockStart { index, kind } => {
                let item = match kind {
                    BlockKind::Thinking => json!({
                        "type": "reasoning",
                        "id": format!("rs_{}", self.id),
                        "summary": [],
                    }),
                    BlockKind::ToolUse => json!({
                        "type": "function_call",
                        "id": format!("fc_{}", self.id),
                        "call_id": format!("call_{}", self.id),
                        "name": "",
                        "arguments": "",
                        "status": "in_progress",
                    }),
                    BlockKind::Text => json!({
                        "type": "message",
                        "id": format!("msg_{}", self.id),
                        "role": "assistant",
                        "status": "in_progress",
                        "content": [],
                    }),
                };
                self.item_index = *index;
                frames.push(self.event(
                    "response.output_item.added",
                    json!({"output_index": self.output_index, "item": item}),
                ));
            }
            StreamEvent::TextDelta { text, .. } => {
                if !self.text_open {
                    self.text_open = true;
                    frames.push(self.event(
                        "response.content_part.added",
                        json!({
                            "output_index": self.output_index,
                            "item_id": format!("msg_{}", self.id),
                            "content_index": 0,
                            "part": {"type": "output_text", "text": "", "annotations": []},
                        }),
                    ));
                }
                self.text.push_str(text);
                frames.push(self.event(
                    "response.output_text.delta",
                    json!({
                        "output_index": self.output_index,
                        "item_id": format!("msg_{}", self.id),
                        "content_index": 0,
                        "delta": text,
                    }),
                ));
            }
            StreamEvent::ThinkingDelta { text, .. } => {
                self.text.push_str(text);
                frames.push(self.event(
                    "response.reasoning_summary_text.delta",
                    json!({
                        "output_index": self.output_index,
                        "item_id": format!("rs_{}", self.id),
                        "summary_index": 0,
                        "delta": text,
                    }),
                ));
            }
            StreamEvent::ThinkingSignature { .. } => {}
            StreamEvent::ToolUseStart { index, id, name } => {
                self.tool_items.push((*index, id.clone(), name.clone()));
                frames.push(self.event(
                    "response.output_item.added",
                    json!({
                        "output_index": self.output_index,
                        "item": {
                            "type": "function_call",
                            "id": format!("fc_{id}"),
                            "call_id": id,
                            "name": name,
                            "arguments": "",
                            "status": "in_progress",
                        }
                    }),
                ));
                self.output_index += 1;
            }
            StreamEvent::ToolInputDelta {
                index,
                partial_json,
            } => {
                frames.push(self.event(
                    "response.function_call_arguments.delta",
                    json!({
                        "output_index": self.output_index,
                        "item_id": format!("fc_{}", self.id),
                        "delta": partial_json,
                        "sequence_number": self.sequence + 1,
                    }),
                ));
                let _ = index;
            }
            StreamEvent::ContentBlockStop { .. } => {
                frames.push(self.event(
                    "response.output_item.done",
                    json!({
                        "output_index": self.output_index,
                        "item": {"type": "message", "status": "completed"},
                    }),
                ));
                self.output_index += 1;
            }
            StreamEvent::MessageDelta { stop_reason, usage } => {
                self.usage = *usage;
                let status = match stop_reason {
                    Some(StopReason::MaxTokens) => "incomplete",
                    _ => "completed",
                };
                frames.push(self.event(
                    if status == "incomplete" {
                        "response.incomplete"
                    } else {
                        "response.completed"
                    },
                    json!({
                        "response": {
                            "id": self.id,
                            "object": "response",
                            "status": status,
                            "model": self.model,
                            "output": [],
                            "output_text": self.text,
                            "usage": {
                                "input_tokens": self.usage.input_tokens,
                                "output_tokens": self.usage.output_tokens,
                                "total_tokens": self.usage.total(),
                                "input_tokens_details": {"cached_tokens": self.usage.cache_read_tokens},
                                "output_tokens_details": {"reasoning_tokens": self.usage.reasoning_tokens},
                            }
                        }
                    }),
                ));
            }
            StreamEvent::MessageStop => {
                if !self.stopped {
                    self.stopped = true;
                }
            }
            StreamEvent::Error { message } => {
                frames.push(self.event(
                    "error",
                    json!({"message": message, "type": "error", "code": "upstream_error"}),
                ));
            }
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The Responses API speaks "input" and "max_output_tokens", not the
    // chat dialect; the parser must map both.
    #[test]
    fn the_responses_dialect_maps_to_the_canonical_request() {
        let wire = json!({
            "model": "gpt-5.2",
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "hello"}]}],
            "max_output_tokens": 256,
            "temperature": 0.3,
            "reasoning": {"effort": "high"}
        });
        let req = parse_request(&wire).unwrap();
        assert_eq!(req.model, "gpt-5.2");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].text(), "hello");
        assert_eq!(req.max_tokens, Some(256));
        assert!((req.temperature.unwrap() - 0.3).abs() < 1e-9);
        assert_eq!(req.reasoning_effort, Some(ReasoningEffort::High));
        // An unknown effort is dropped rather than misread.
        let odd = parse_request(&json!({
            "model": "m",
            "input": "plain string input",
            "reasoning": {"effort": "bogus"}
        }))
        .unwrap();
        assert_eq!(odd.reasoning_effort, None);
        assert!(
            !odd.messages.is_empty(),
            "a plain-string input still parses"
        );
    }

    // A Responses-shaped reply parses back with usage and text intact.
    #[test]
    fn a_responses_reply_parses_back_to_text_and_usage() {
        let wire = json!({
            "id": "resp_1",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.2",
            "output": [
                {"type": "reasoning", "content": [{"type": "reasoning_text", "text": "hmm"}]},
                {"type": "message", "content": [{"type": "output_text", "text": "the answer"}]}
            ],
            "usage": {
                "input_tokens": 9,
                "output_tokens": 4,
                "output_tokens_details": {"reasoning_tokens": 3}
            }
        });
        let resp = parse_response(&wire).unwrap();
        assert_eq!(resp.text(), "the answer");
        assert_eq!(resp.usage.input_tokens, 9);
        assert_eq!(resp.usage.output_tokens, 4);
        assert_eq!(resp.usage.reasoning_tokens, 3);
    }

    // format_response output is accepted by parse_response again.
    #[test]
    fn a_formatted_response_round_trips() {
        let resp = ChatResponse {
            id: "resp_2".into(),
            model: "gpt-5.2".into(),
            content: vec![ContentBlock::text("round trip")],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage {
                input_tokens: 2,
                output_tokens: 3,
                ..Usage::default()
            },
        };
        let wire = format_response(&resp, "gpt-5.2");
        let back = parse_response(&wire).unwrap();
        assert_eq!(back.text(), "round trip");
    }
}
