//! Anthropic Messages: `POST /v1/messages`.
//!
//! The shape Claude Code speaks, and the one with the richest block model,
//! so it doubles as the reference for the canonical types.

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
        .get("max_tokens")
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    req.temperature = obj.get("temperature").and_then(Value::as_f64);
    req.top_p = obj.get("top_p").and_then(Value::as_f64);
    req.top_k = obj.get("top_k").and_then(Value::as_u64).map(|v| v as u32);
    if let Some(stop) = obj.get("stop_sequences").and_then(Value::as_array) {
        for s in stop {
            if let Some(s) = s.as_str() {
                req.stop.push(s.to_string());
            }
        }
    }

    // System prompt.
    match obj.get("system") {
        Some(Value::String(s)) => req.system.push(ContentBlock::text(s.clone())),
        Some(Value::Array(blocks)) => {
            for b in blocks {
                match b.get("type").and_then(Value::as_str).unwrap_or("text") {
                    "text" => {
                        if let Some(t) = b.get("text").and_then(Value::as_str) {
                            req.system.push(ContentBlock::text(t));
                        }
                    }
                    "cache_control" => {}
                    _ => {}
                }
            }
        }
        _ => {}
    }

    // Tools.
    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for t in tools {
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
                    .get("input_schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            });
        }
    }
    req.tool_choice = match obj.get("tool_choice") {
        Some(Value::Object(m)) => match m.get("type").and_then(Value::as_str) {
            Some("auto") => Some(ToolChoice::Auto),
            Some("any") => Some(ToolChoice::Required),
            Some("tool") => Some(ToolChoice::Tool {
                name: m
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            _ => None,
        },
        Some(Value::String(s)) => match s.as_str() {
            "auto" => Some(ToolChoice::Auto),
            "any" => Some(ToolChoice::Required),
            "none" => Some(ToolChoice::None),
            other => Some(ToolChoice::Tool {
                name: other.to_string(),
            }),
        },
        _ => None,
    };

    // Messages.
    let messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::BadRequest("the request has no messages".into()))?;
    for m in messages {
        let role = match m.get("role").and_then(Value::as_str).unwrap_or("user") {
            "assistant" => Role::Assistant,
            _ => Role::User,
        };
        let mut blocks = Vec::new();
        match m.get("content") {
            Some(Value::String(s)) => blocks.push(ContentBlock::text(s.clone())),
            Some(Value::Array(parts)) => {
                for p in parts {
                    blocks.push(parse_block(p));
                }
            }
            _ => {}
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

fn parse_block(p: &Value) -> ContentBlock {
    match p.get("type").and_then(Value::as_str).unwrap_or("text") {
        "text" => ContentBlock::text(
            p.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        "thinking" => ContentBlock::Thinking {
            text: p
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            signature: p
                .get("signature")
                .and_then(Value::as_str)
                .map(|s| s.to_string()),
        },
        "redacted_thinking" => ContentBlock::Thinking {
            text: String::new(),
            signature: p.get("data").and_then(Value::as_str).map(|s| s.to_string()),
        },
        "tool_use" => ContentBlock::ToolUse {
            id: p
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: p
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input: p.get("input").cloned().unwrap_or_else(|| json!({})),
        },
        "tool_result" => {
            let inner = match p.get("content") {
                Some(Value::String(s)) => vec![ContentBlock::text(s.clone())],
                Some(Value::Array(items)) => items.iter().map(parse_block).collect(),
                _ => Vec::new(),
            };
            ContentBlock::ToolResult {
                tool_use_id: p
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                content: inner,
                is_error: p.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            }
        }
        "image" => {
            let source = p.get("source").cloned().unwrap_or(Value::Null);
            let media_type = source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string();
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .or_else(|| source.get("url").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string();
            ContentBlock::Image { media_type, data }
        }
        "document" => {
            let source = p.get("source").cloned().unwrap_or(Value::Null);
            ContentBlock::Document {
                media_type: source
                    .get("media_type")
                    .and_then(Value::as_str)
                    .unwrap_or("application/pdf")
                    .to_string(),
                data: source
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: p
                    .get("title")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
            }
        }
        _ => ContentBlock::text(String::new()),
    }
}

pub fn parse_response(v: &Value) -> ApiResult<ChatResponse> {
    let obj = v
        .as_object()
        .ok_or_else(|| ApiError::Upstream("the reply was not an object".into()))?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("msg_unknown")
        .to_string();
    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut content = Vec::new();
    if let Some(blocks) = obj.get("content").and_then(Value::as_array) {
        for b in blocks {
            content.push(parse_block(b));
        }
    }

    let stop_reason = match obj.get("stop_reason").and_then(Value::as_str) {
        Some("end_turn") => Some(StopReason::EndTurn),
        Some("max_tokens") => Some(StopReason::MaxTokens),
        Some("stop_sequence") => Some(StopReason::StopSequence),
        Some("tool_use") => Some(StopReason::ToolUse),
        _ => None,
    };

    let usage = obj
        .get("usage")
        .map(|u| Usage {
            input_tokens: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            output_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            cache_read_tokens: u
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_write_tokens: u
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            reasoning_tokens: 0,
        })
        .unwrap_or_default();

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

    // Anthropic requires max_tokens.
    let max = req.max_tokens.unwrap_or(8192);
    body.insert("max_tokens".into(), json!(max));

    if !req.system.is_empty() {
        let text: Vec<String> = req
            .system
            .iter()
            .map(|b| b.plain_text())
            .filter(|t| !t.is_empty())
            .collect();
        if !text.is_empty() {
            body.insert("system".into(), json!(text.join("\n\n")));
        }
    }

    let mut messages = Vec::new();
    for m in &req.messages {
        let blocks: Vec<Value> = m.content.iter().map(format_block).collect();
        if blocks.is_empty() {
            continue;
        }
        let role = match m.role {
            Role::Assistant => "assistant",
            _ => "user",
        };
        messages.push(json!({"role": role, "content": blocks}));
    }
    body.insert("messages".into(), Value::Array(messages));

    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        body.insert("top_p".into(), json!(p));
    }
    if let Some(k) = req.top_k {
        body.insert("top_k".into(), json!(k));
    }
    if !req.stop.is_empty() {
        body.insert("stop_sequences".into(), json!(req.stop));
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description.clone().unwrap_or_default(),
                    "input_schema": t.input_schema.clone(),
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }
    if let Some(choice) = &req.tool_choice {
        body.insert(
            "tool_choice".into(),
            match choice {
                ToolChoice::Auto => json!({"type": "auto"}),
                ToolChoice::None => json!({"type": "none"}),
                ToolChoice::Required => json!({"type": "any"}),
                ToolChoice::Tool { name } => json!({"type": "tool", "name": name}),
            },
        );
    }
    if req.stream {
        body.insert("stream".into(), json!(true));
    }
    // Thinking maps onto Anthropic's own knob.
    if let Some(effort) = req.reasoning_effort {
        if let Some(budget) = effort.budget_tokens() {
            if budget > 0 {
                body.insert(
                    "thinking".into(),
                    json!({"type": "enabled", "budget_tokens": budget}),
                );
            } else {
                body.insert("thinking".into(), json!({"type": "disabled"}));
            }
        }
    }
    Value::Object(body)
}

fn format_block(b: &ContentBlock) -> Value {
    match b {
        ContentBlock::Text { text } => json!({"type": "text", "text": text}),
        ContentBlock::Thinking { text, signature } => {
            let mut m = Map::new();
            m.insert("type".into(), json!("thinking"));
            m.insert("thinking".into(), json!(text));
            if let Some(sig) = signature {
                m.insert("signature".into(), json!(sig));
            }
            Value::Object(m)
        }
        ContentBlock::ToolUse { id, name, input } => json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let inner: Vec<Value> = content.iter().map(format_block).collect();
            let mut m = Map::new();
            m.insert("type".into(), json!("tool_result"));
            m.insert("tool_use_id".into(), json!(tool_use_id));
            m.insert(
                "content".into(),
                if inner.is_empty() {
                    json!("")
                } else {
                    Value::Array(inner)
                },
            );
            if *is_error {
                m.insert("is_error".into(), json!(true));
            }
            Value::Object(m)
        }
        ContentBlock::Image { media_type, data } => {
            if media_type == "image/url" {
                json!({"type": "text", "text": format!("[image: {data}]")})
            } else {
                json!({
                    "type": "image",
                    "source": {"type": "base64", "media_type": media_type, "data": data},
                })
            }
        }
        ContentBlock::Document {
            media_type,
            data,
            name,
        } => {
            let mut m = Map::new();
            m.insert("type".into(), json!("document"));
            m.insert(
                "source".into(),
                json!({"type": "base64", "media_type": media_type, "data": data}),
            );
            if let Some(n) = name {
                m.insert("title".into(), json!(n));
            }
            Value::Object(m)
        }
    }
}

pub fn format_response(resp: &ChatResponse, model: &str) -> Value {
    let content: Vec<Value> = resp.content.iter().map(format_block).collect();
    let stop = match resp.stop_reason {
        Some(StopReason::EndTurn) => "end_turn",
        Some(StopReason::MaxTokens) => "max_tokens",
        Some(StopReason::StopSequence) => "stop_sequence",
        Some(StopReason::ToolUse) => "tool_use",
        Some(StopReason::Error) => "end_turn",
        None => "end_turn",
    };
    json!({
        "id": resp.id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop,
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": resp.usage.input_tokens,
            "output_tokens": resp.usage.output_tokens,
            "cache_read_input_tokens": resp.usage.cache_read_tokens,
            "cache_creation_input_tokens": resp.usage.cache_write_tokens,
        }
    })
}

// ---------------------------------------------------------------- streams --

/// Reads an Anthropic stream into canonical events.
pub struct Collector {
    pub id: String,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: Option<StopReason>,
    pub started: bool,
    pub finished: bool,
    pub block_kinds: Vec<BlockKind>,
}

impl Collector {
    pub fn new() -> Self {
        Collector {
            id: "msg_unknown".to_string(),
            model: String::new(),
            usage: Usage::default(),
            stop_reason: None,
            started: false,
            finished: false,
            block_kinds: Vec::new(),
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
            "message_start" => {
                let msg = chunk.get("message").cloned().unwrap_or(Value::Null);
                if let Some(id) = msg.get("id").and_then(Value::as_str) {
                    self.id = id.to_string();
                }
                if let Some(model) = msg.get("model").and_then(Value::as_str) {
                    self.model = model.to_string();
                }
                if let Some(u) = msg.get("usage") {
                    self.usage.input_tokens = u
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(self.usage.input_tokens);
                    self.usage.cache_read_tokens = u
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    self.usage.cache_write_tokens = u
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                }
                self.started = true;
                out.push(StreamEvent::MessageStart {
                    id: self.id.clone(),
                    model: self.model.clone(),
                    usage: self.usage,
                });
            }
            "content_block_start" => {
                let index = chunk.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let block = chunk.get("content_block").cloned().unwrap_or(Value::Null);
                let bkind = match block.get("type").and_then(Value::as_str).unwrap_or("text") {
                    "thinking" => BlockKind::Thinking,
                    "tool_use" => BlockKind::ToolUse,
                    _ => BlockKind::Text,
                };
                while self.block_kinds.len() <= index {
                    self.block_kinds.push(BlockKind::Text);
                }
                self.block_kinds[index] = bkind;
                out.push(StreamEvent::ContentBlockStart { index, kind: bkind });
                if bkind == BlockKind::ToolUse {
                    out.push(StreamEvent::ToolUseStart {
                        index,
                        id: block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
                if bkind == BlockKind::Text {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            out.push(StreamEvent::TextDelta {
                                index,
                                text: t.to_string(),
                            });
                        }
                    }
                }
            }
            "content_block_delta" => {
                let index = chunk.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let delta = chunk.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        if let Some(t) = delta.get("text").and_then(Value::as_str) {
                            out.push(StreamEvent::TextDelta {
                                index,
                                text: t.to_string(),
                            });
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                            out.push(StreamEvent::ThinkingDelta {
                                index,
                                text: t.to_string(),
                            });
                        }
                    }
                    "signature_delta" => {
                        if let Some(s) = delta.get("signature").and_then(Value::as_str) {
                            out.push(StreamEvent::ThinkingSignature {
                                index,
                                signature: s.to_string(),
                            });
                        }
                    }
                    "input_json_delta" => {
                        if let Some(p) = delta.get("partial_json").and_then(Value::as_str) {
                            out.push(StreamEvent::ToolInputDelta {
                                index,
                                partial_json: p.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = chunk.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                out.push(StreamEvent::ContentBlockStop { index });
            }
            "message_delta" => {
                if let Some(reason) = chunk
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = match reason {
                        "end_turn" => Some(StopReason::EndTurn),
                        "max_tokens" => Some(StopReason::MaxTokens),
                        "stop_sequence" => Some(StopReason::StopSequence),
                        "tool_use" => Some(StopReason::ToolUse),
                        _ => None,
                    };
                }
                if let Some(u) = chunk.get("usage") {
                    if let Some(n) = u.get("output_tokens").and_then(Value::as_u64) {
                        self.usage.output_tokens = n;
                    }
                    if let Some(n) = u.get("input_tokens").and_then(Value::as_u64) {
                        self.usage.input_tokens = n;
                    }
                }
            }
            "message_stop" => {}
            "error" => {
                let message = chunk
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("the upstream failed");
                out.push(StreamEvent::Error {
                    message: message.to_string(),
                });
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
        out.push(StreamEvent::MessageDelta {
            stop_reason: self.stop_reason,
            usage: self.usage,
        });
        out.push(StreamEvent::MessageStop);
    }
}

/// Renders canonical events back into an Anthropic stream.
pub struct Renderer {
    pub id: String,
    pub model: String,
    pub opened: Vec<bool>,
    pub tool_ids: Vec<(usize, String)>,
    pub usage: Usage,
    pub started: bool,
    pub stopped: bool,
}

impl Renderer {
    pub fn new(id: String, model: String) -> Self {
        Renderer {
            id,
            model,
            opened: Vec::new(),
            tool_ids: Vec::new(),
            usage: Usage::default(),
            started: false,
            stopped: false,
        }
    }

    fn ensure_open(&mut self, index: usize, kind: BlockKind, out: &mut Vec<SseFrame>) {
        while self.opened.len() <= index {
            self.opened.push(false);
        }
        if self.opened[index] {
            return;
        }
        // A tool call opens when its id and name arrive, not before.
        if kind == BlockKind::ToolUse {
            return;
        }
        self.opened[index] = true;
        let block = match kind {
            BlockKind::Text => json!({"type": "text", "text": ""}),
            BlockKind::Thinking => json!({"type": "thinking", "thinking": ""}),
            BlockKind::ToolUse => json!({"type": "tool_use", "id": "", "name": "", "input": {}}),
        };
        out.push(SseFrame::named(
            "content_block_start",
            json!({"type": "content_block_start", "index": index, "content_block": block})
                .to_string(),
        ));
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
                frames.push(SseFrame::named(
                    "message_start",
                    json!({
                        "type": "message_start",
                        "message": {
                            "id": self.id,
                            "type": "message",
                            "role": "assistant",
                            "model": self.model,
                            "content": [],
                            "stop_reason": Value::Null,
                            "stop_sequence": Value::Null,
                            "usage": {
                                "input_tokens": self.usage.input_tokens,
                                "output_tokens": self.usage.output_tokens,
                                "cache_read_input_tokens": self.usage.cache_read_tokens,
                                "cache_creation_input_tokens": self.usage.cache_write_tokens,
                            }
                        }
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ContentBlockStart { index, kind } => {
                self.ensure_open(*index, *kind, &mut frames);
            }
            StreamEvent::TextDelta { index, text } => {
                self.ensure_open(*index, BlockKind::Text, &mut frames);
                frames.push(SseFrame::named(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "text_delta", "text": text},
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ThinkingDelta { index, text } => {
                self.ensure_open(*index, BlockKind::Thinking, &mut frames);
                frames.push(SseFrame::named(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "thinking_delta", "thinking": text},
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ThinkingSignature { index, signature } => {
                frames.push(SseFrame::named(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "signature_delta", "signature": signature},
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ToolUseStart { index, id, name } => {
                while self.opened.len() <= *index {
                    self.opened.push(false);
                }
                if self.opened[*index] {
                    return frames;
                }
                self.opened[*index] = true;
                self.tool_ids.push((*index, id.clone()));
                frames.push(SseFrame::named(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}},
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ToolInputDelta {
                index,
                partial_json,
            } => {
                frames.push(SseFrame::named(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "input_json_delta", "partial_json": partial_json},
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ContentBlockStop { index } => {
                if self.opened.get(*index).copied().unwrap_or(false) {
                    frames.push(SseFrame::named(
                        "content_block_stop",
                        json!({"type": "content_block_stop", "index": index}).to_string(),
                    ));
                    self.opened[*index] = false;
                }
            }
            StreamEvent::MessageDelta { stop_reason, usage } => {
                self.usage = *usage;
                let stop = match stop_reason {
                    Some(StopReason::MaxTokens) => "max_tokens",
                    Some(StopReason::StopSequence) => "stop_sequence",
                    Some(StopReason::ToolUse) => "tool_use",
                    _ => "end_turn",
                };
                frames.push(SseFrame::named(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": stop, "stop_sequence": Value::Null},
                        "usage": {"output_tokens": self.usage.output_tokens},
                    })
                    .to_string(),
                ));
            }
            StreamEvent::MessageStop => {
                if !self.stopped {
                    self.stopped = true;
                    frames.push(SseFrame::named(
                        "message_stop",
                        "{\"type\":\"message_stop\"}",
                    ));
                }
            }
            StreamEvent::Error { message } => {
                frames.push(SseFrame::named(
                    "error",
                    json!({
                        "type": "error",
                        "error": {"type": "api_error", "message": message},
                    })
                    .to_string(),
                ));
            }
        }
        frames
    }
}
