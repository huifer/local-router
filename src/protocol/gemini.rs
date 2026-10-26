//! Google Gemini: `POST /v1beta/models/{model}:generateContent`.
//!
//! Gemini keeps the system prompt outside the conversation and calls tools
//! by name rather than by id, so both need care when translating.

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
        stream: obj
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..Default::default()
    };

    if let Some(si) = obj.get("systemInstruction").or_else(|| obj.get("system_instruction")) {
        if let Some(parts) = si.get("parts").and_then(Value::as_array) {
            for p in parts {
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    req.system.push(ContentBlock::text(t));
                }
            }
        }
    }

    if let Some(gc) = obj.get("generationConfig").or_else(|| obj.get("generation_config")) {
        req.max_tokens = gc
            .get("maxOutputTokens")
            .and_then(Value::as_u64)
            .map(|v| v as u32);
        req.temperature = gc.get("temperature").and_then(Value::as_f64);
        req.top_p = gc.get("topP").and_then(Value::as_f64);
        req.top_k = gc.get("topK").and_then(Value::as_u64).map(|v| v as u32);
        if let Some(stop) = gc.get("stopSequences").and_then(Value::as_array) {
            for s in stop {
                if let Some(s) = s.as_str() {
                    req.stop.push(s.to_string());
                }
            }
        }
        if let Some(tc) = gc.get("thinkingConfig") {
            let budget = tc.get("thinkingBudget").and_then(Value::as_u64).unwrap_or(0);
            req.reasoning_effort = Some(match budget {
                0 => ReasoningEffort::None,
                1..=2048 => ReasoningEffort::Low,
                2049..=8192 => ReasoningEffort::Medium,
                8193..=24576 => ReasoningEffort::High,
                _ => ReasoningEffort::Max,
            });
        }
    }

    // Tools.
    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for t in tools {
            if let Some(decls) = t.get("functionDeclarations").and_then(Value::as_array) {
                for d in decls {
                    let name = d.get("name").and_then(Value::as_str).unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    req.tools.push(ToolDef {
                        name: name.to_string(),
                        description: d
                            .get("description")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string()),
                        input_schema: d
                            .get("parameters")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    });
                }
            }
        }
    }
    req.tool_choice = obj
        .get("toolConfig")
        .and_then(|tc| tc.get("functionCallingConfig"))
        .and_then(|fc| fc.get("mode"))
        .and_then(Value::as_str)
        .map(|mode| match mode {
            "ANY" => ToolChoice::Required,
            "NONE" => ToolChoice::None,
            "AUTO" => ToolChoice::Auto,
            _ => ToolChoice::Auto,
        });

    // Contents.
    let contents = obj
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::BadRequest("the request has no contents".into()))?;
    for c in contents {
        let role = match c.get("role").and_then(Value::as_str) {
            Some("model") | Some("assistant") => Role::Assistant,
            _ => Role::User,
        };
        let mut blocks = Vec::new();
        if let Some(parts) = c.get("parts").and_then(Value::as_array) {
            for p in parts {
                blocks.push(parse_part(p));
            }
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

fn parse_part(p: &Value) -> ContentBlock {
    if let Some(text) = p.get("text").and_then(Value::as_str) {
        if p.get("thought").and_then(Value::as_bool).unwrap_or(false) {
            return ContentBlock::Thinking {
                text: text.to_string(),
                signature: None,
            };
        }
        return ContentBlock::text(text);
    }
    if let Some(fc) = p.get("functionCall") {
        return ContentBlock::ToolUse {
            id: fc
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: fc.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            input: fc.get("args").cloned().unwrap_or_else(|| json!({})),
        };
    }
    if let Some(fr) = p.get("functionResponse") {
        let name = fr.get("name").and_then(Value::as_str).unwrap_or_default();
        let response = fr.get("response").cloned().unwrap_or(Value::Null);
        let text = match response {
            Value::String(s) => s,
            other => other.to_string(),
        };
        return ContentBlock::ToolResult {
            tool_use_id: name.to_string(),
            content: vec![ContentBlock::text(text)],
            is_error: false,
        };
    }
    if let Some(inline) = p.get("inlineData").or_else(|| p.get("inline_data")) {
        return ContentBlock::Image {
            media_type: inline
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string(),
            data: inline
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
    }
    ContentBlock::text(String::new())
}

pub fn parse_response(v: &Value) -> ApiResult<ChatResponse> {
    let obj = v
        .as_object()
        .ok_or_else(|| ApiError::Upstream("the reply was not an object".into()))?;
    let model = obj
        .get("modelVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut content = Vec::new();
    let mut stop_reason = None;

    if let Some(candidate) = obj
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
    {
        if let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
        {
            for p in parts {
                content.push(parse_part(p));
            }
        }
        stop_reason = match candidate.get("finishReason").and_then(Value::as_str) {
            Some("MAX_TOKENS") => Some(StopReason::MaxTokens),
            Some("STOP") => {
                if content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
                {
                    Some(StopReason::ToolUse)
                } else {
                    Some(StopReason::EndTurn)
                }
            }
            Some("SAFETY") | Some("RECITATION") | Some("PROHIBITED_CONTENT") => {
                Some(StopReason::Error)
            }
            _ => None,
        };
    }

    let u = obj.get("usageMetadata");
    let usage = Usage {
        input_tokens: u
            .and_then(|u| u.get("promptTokenCount"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: u
            .and_then(|u| u.get("candidatesTokenCount"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens: u
            .and_then(|u| u.get("cachedContentTokenCount"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_write_tokens: 0,
        reasoning_tokens: u
            .and_then(|u| u.get("thoughtsTokenCount"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    };

    Ok(ChatResponse {
        id: obj
            .get("responseId")
            .and_then(Value::as_str)
            .unwrap_or("gemini-unknown")
            .to_string(),
        model,
        content,
        stop_reason,
        usage,
    })
}

// -------------------------------------------------------------- formatting --

pub fn format_request(req: &ChatRequest, model: &str) -> Value {
    let mut body = Map::new();

    let system_text = req
        .system
        .iter()
        .map(|b| b.plain_text())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system_text.is_empty() {
        body.insert(
            "systemInstruction".to_string(),
            json!({"parts": [{"text": system_text}]}),
        );
    }

    let mut contents = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();
    for m in &req.messages {
        match m.role {
            Role::System => {}
            Role::User => {
                let mut parts = Vec::new();
                for b in &m.content {
                    match b {
                        ContentBlock::Text { text } => {
                            parts.push(json!({"text": text}));
                        }
                        ContentBlock::Image { media_type, data } => {
                            if media_type == "image/url" {
                                parts.push(json!({"text": format!("[image: {data}]")}));
                            } else {
                                parts.push(json!({
                                    "inlineData": {"mimeType": media_type, "data": data}
                                }));
                            }
                        }
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "user", "parts": parts}));
                }
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                for b in &m.content {
                    match b {
                        ContentBlock::Text { text } => parts.push(json!({"text": text})),
                        ContentBlock::Thinking { text, .. } => {
                            parts.push(json!({"text": text, "thought": true}))
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            parts.push(json!({"functionCall": {"name": name, "args": input}}));
                        }
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
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
                        pending_tool_results.push(json!({
                            "functionResponse": {"name": tool_use_id, "response": {"result": text}}
                        }));
                    }
                }
            }
        }
    }
    if !pending_tool_results.is_empty() {
        contents.push(json!({"role": "user", "parts": pending_tool_results}));
    }
    body.insert("contents".into(), Value::Array(contents));

    let mut gc = Map::new();
    if let Some(n) = req.max_tokens {
        gc.insert("maxOutputTokens".into(), json!(n));
    }
    if let Some(t) = req.temperature {
        gc.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p {
        gc.insert("topP".into(), json!(p));
    }
    if let Some(k) = req.top_k {
        gc.insert("topK".into(), json!(k));
    }
    if !req.stop.is_empty() {
        gc.insert("stopSequences".into(), json!(req.stop));
    }
    if let Some(effort) = req.reasoning_effort {
        let budget = match effort {
            ReasoningEffort::None => 0,
            ReasoningEffort::Minimal => 1024,
            ReasoningEffort::Low => 4096,
            ReasoningEffort::Medium => 8192,
            ReasoningEffort::High => 16384,
            ReasoningEffort::Max => 32768,
        };
        gc.insert(
            "thinkingConfig".into(),
            json!({"thinkingBudget": budget, "includeThoughts": true}),
        );
    }
    if !gc.is_empty() {
        body.insert("generationConfig".into(), Value::Object(gc));
    }

    if !req.tools.is_empty() {
        let decls: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description.clone().unwrap_or_default(),
                    "parameters": t.input_schema.clone(),
                })
            })
            .collect();
        body.insert(
            "tools".into(),
            json!([{"functionDeclarations": decls}]),
        );
    }
    if let Some(choice) = &req.tool_choice {
        let mode = match choice {
            ToolChoice::Auto => "AUTO",
            ToolChoice::None => "NONE",
            ToolChoice::Required => "ANY",
            ToolChoice::Tool { .. } => "ANY",
        };
        body.insert(
            "toolConfig".into(),
            json!({"functionCallingConfig": {"mode": mode}}),
        );
    }
    let _ = model;
    Value::Object(body)
}

pub fn format_response(resp: &ChatResponse, model: &str) -> Value {
    let mut parts = Vec::new();
    for b in &resp.content {
        match b {
            ContentBlock::Text { text } => parts.push(json!({"text": text})),
            ContentBlock::Thinking { text, .. } => {
                parts.push(json!({"text": text, "thought": true}))
            }
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(json!({"functionCall": {"name": name, "args": input}}));
            }
            _ => {}
        }
    }
    let finish = match resp.stop_reason {
        Some(StopReason::MaxTokens) => "MAX_TOKENS",
        Some(StopReason::Error) => "SAFETY",
        _ => "STOP",
    };
    json!({
        "candidates": [{
            "content": {"role": "model", "parts": parts},
            "finishReason": finish,
            "index": 0,
        }],
        "usageMetadata": {
            "promptTokenCount": resp.usage.input_tokens,
            "candidatesTokenCount": resp.usage.output_tokens,
            "totalTokenCount": resp.usage.total(),
            "cachedContentTokenCount": resp.usage.cache_read_tokens,
            "thoughtsTokenCount": resp.usage.reasoning_tokens,
        },
        "modelVersion": model,
        "responseId": resp.id,
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
    pub tool_names: Vec<(usize, String)>,
}

impl Collector {
    pub fn new() -> Self {
        Collector {
            id: "gemini-unknown".to_string(),
            model: String::new(),
            usage: Usage::default(),
            stop_reason: None,
            started: false,
            finished: false,
            next_index: 0,
            open_block: None,
            tool_names: Vec::new(),
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
        if !self.started {
            self.started = true;
            if let Some(m) = chunk.get("modelVersion").and_then(Value::as_str) {
                self.model = m.to_string();
            }
            if let Some(id) = chunk.get("responseId").and_then(Value::as_str) {
                self.id = id.to_string();
            }
            out.push(StreamEvent::MessageStart {
                id: self.id.clone(),
                model: self.model.clone(),
                usage: self.usage,
            });
        }
        if let Some(u) = chunk.get("usageMetadata") {
            self.usage.input_tokens = u
                .get("promptTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(self.usage.input_tokens);
            self.usage.output_tokens = u
                .get("candidatesTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(self.usage.output_tokens);
            self.usage.cache_read_tokens = u
                .get("cachedContentTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(self.usage.cache_read_tokens);
            self.usage.reasoning_tokens = u
                .get("thoughtsTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(self.usage.reasoning_tokens);
        }

        let Some(candidate) = chunk
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return;
        };

        if let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
        {
            for p in parts {
                if let Some(text) = p.get("text").and_then(Value::as_str) {
                    if p.get("thought").and_then(Value::as_bool).unwrap_or(false) {
                        let index = match self.open_block {
                            Some((i, BlockKind::Thinking)) => i,
                            _ => {
                                let i = self.next_index;
                                self.next_index += 1;
                                out.push(StreamEvent::ContentBlockStart {
                                    index: i,
                                    kind: BlockKind::Thinking,
                                });
                                self.open_block = Some((i, BlockKind::Thinking));
                                i
                            }
                        };
                        out.push(StreamEvent::ThinkingDelta {
                            index,
                            text: text.to_string(),
                        });
                    } else {
                        let index = match self.open_block {
                            Some((i, BlockKind::Text)) => i,
                            _ => {
                                let i = self.next_index;
                                self.next_index += 1;
                                out.push(StreamEvent::ContentBlockStart {
                                    index: i,
                                    kind: BlockKind::Text,
                                });
                                self.open_block = Some((i, BlockKind::Text));
                                i
                            }
                        };
                        out.push(StreamEvent::TextDelta {
                            index,
                            text: text.to_string(),
                        });
                    }
                } else if let Some(fc) = p.get("functionCall") {
                    let index = self.next_index;
                    self.next_index += 1;
                    let name = fc.get("name").and_then(Value::as_str).unwrap_or_default();
                    out.push(StreamEvent::ContentBlockStart {
                        index,
                        kind: BlockKind::ToolUse,
                    });
                    out.push(StreamEvent::ToolUseStart {
                        index,
                        id: name.to_string(),
                        name: name.to_string(),
                    });
                    self.tool_names.push((index, name.to_string()));
                    let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                    out.push(StreamEvent::ToolInputDelta {
                        index,
                        partial_json: serde_json::to_string(&args).unwrap_or_else(|_| "{}".into()),
                    });
                    out.push(StreamEvent::ContentBlockStop { index });
                }
            }
        }

        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.stop_reason = match reason {
                "MAX_TOKENS" => Some(StopReason::MaxTokens),
                "STOP" => Some(StopReason::EndTurn),
                "SAFETY" | "RECITATION" | "PROHIBITED_CONTENT" => Some(StopReason::Error),
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

/// Renders canonical events back into a Gemini stream.
pub struct Renderer {
    pub id: String,
    pub model: String,
    pub usage: Usage,
    pub finished: bool,
}

impl Renderer {
    pub fn new(id: String, model: String) -> Self {
        Renderer {
            id,
            model,
            usage: Usage::default(),
            finished: false,
        }
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
            }
            StreamEvent::ContentBlockStart { .. } => {}
            StreamEvent::TextDelta { text, .. } => {
                frames.push(SseFrame::data(
                    json!({
                        "candidates": [{
                            "content": {"role": "model", "parts": [{"text": text}]},
                            "index": 0,
                        }],
                        "modelVersion": self.model,
                        "responseId": self.id,
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ThinkingDelta { text, .. } => {
                frames.push(SseFrame::data(
                    json!({
                        "candidates": [{
                            "content": {"role": "model", "parts": [{"text": text, "thought": true}]},
                            "index": 0,
                        }],
                        "modelVersion": self.model,
                        "responseId": self.id,
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ToolUseStart { id, name, .. } => {
                frames.push(SseFrame::data(
                    json!({
                        "candidates": [{
                            "content": {"role": "model", "parts": [{"functionCall": {"name": name, "args": {}}}]},
                            "index": 0,
                        }],
                        "modelVersion": self.model,
                        "responseId": id,
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ToolInputDelta { partial_json, .. } => {
                let args: Value = serde_json::from_str(partial_json).unwrap_or_else(|_| json!({}));
                frames.push(SseFrame::data(
                    json!({
                        "candidates": [{
                            "content": {"role": "model", "parts": [{"functionCall": {"name": "", "args": args}}]},
                            "index": 0,
                        }],
                        "modelVersion": self.model,
                        "responseId": self.id,
                    })
                    .to_string(),
                ));
            }
            StreamEvent::ContentBlockStop { .. } => {}
            StreamEvent::ThinkingSignature { .. } => {}
            StreamEvent::MessageDelta { stop_reason, usage } => {
                self.usage = *usage;
                let finish = match stop_reason {
                    Some(StopReason::MaxTokens) => "MAX_TOKENS",
                    Some(StopReason::Error) => "SAFETY",
                    _ => "STOP",
                };
                frames.push(SseFrame::data(
                    json!({
                        "candidates": [{
                            "content": {"role": "model", "parts": []},
                            "finishReason": finish,
                            "index": 0,
                        }],
                        "usageMetadata": {
                            "promptTokenCount": self.usage.input_tokens,
                            "candidatesTokenCount": self.usage.output_tokens,
                            "totalTokenCount": self.usage.total(),
                            "cachedContentTokenCount": self.usage.cache_read_tokens,
                            "thoughtsTokenCount": self.usage.reasoning_tokens,
                        },
                        "modelVersion": self.model,
                        "responseId": self.id,
                    })
                    .to_string(),
                ));
            }
            StreamEvent::MessageStop => {
                self.finished = true;
            }
            StreamEvent::Error { message } => {
                frames.push(SseFrame::data(
                    json!({"error": {"code": 500, "message": message, "status": "INTERNAL"}}).to_string(),
                ));
            }
        }
        frames
    }
}
