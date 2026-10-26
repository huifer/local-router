//! The protocol translations, both ways.

use local_router::gateway::client::StreamCollector;
use local_router::protocol::anthropic;
use local_router::protocol::gemini;
use local_router::protocol::ir::*;
use local_router::protocol::openai_chat;
use local_router::protocol::openai_responses;
use local_router::protocol::sse;

fn chat_request() -> ChatRequest {
    ChatRequest {
        model: "mock/mock-small".into(),
        system: vec![ContentBlock::text("be brief")],
        messages: vec![
            Message::user("hello"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::text("hi there")],
                tool_call_id: None,
                name: None,
            },
            Message::user("what is two plus two"),
        ],
        tools: vec![ToolDef::new(
            "calculator",
            Some("add two numbers".into()),
            serde_json::json!({
                "type": "object",
                "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
                "required": ["a", "b"],
            }),
        )],
        tool_choice: Some(ToolChoice::Auto),
        max_tokens: Some(256),
        temperature: Some(0.4),
        stream: false,
        ..Default::default()
    }
}

// ------------------------------------------------------ anthropic shapes --

#[test]
fn anthropic_request_parses() {
    let raw = serde_json::json!({
        "model": "mock/mock-small",
        "max_tokens": 100,
        "system": "be brief",
        "stream": false,
        "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "hello"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "QUJD"}},
            ]},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "calculator", "input": {"a": 2, "b": 2}},
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": [{"type": "text", "text": "4"}]},
            ]},
        ],
        "tools": [
            {"name": "calculator", "description": "add", "input_schema": {"type": "object", "properties": {}}},
        ],
        "tool_choice": {"type": "auto"},
    });
    let req = anthropic::parse_request(&raw).unwrap();
    assert_eq!(req.model, "mock/mock-small");
    assert_eq!(req.system.len(), 1);
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.tools.len(), 1);
    assert_eq!(req.max_tokens, Some(100));

    match &req.messages[0].content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "hello"),
        other => panic!("expected text, got {other:?}"),
    }
    match &req.messages[0].content[1] {
        ContentBlock::Image { media_type, data } => {
            assert_eq!(media_type, "image/png");
            assert_eq!(data, "QUJD");
        }
        other => panic!("expected image, got {other:?}"),
    }
    match &req.messages[1].content[0] {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "toolu_1");
            assert_eq!(name, "calculator");
            assert_eq!(input["a"], 2);
        }
        other => panic!("expected tool use, got {other:?}"),
    }
    match &req.messages[2].content[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } => {
            assert_eq!(tool_use_id, "toolu_1");
            assert_eq!(content.len(), 1);
        }
        other => panic!("expected tool result, got {other:?}"),
    }
}

#[test]
fn anthropic_request_formats_back() {
    let req = chat_request();
    let body = anthropic::format_request(&req, "upstream-model");
    assert_eq!(body["model"], "upstream-model");
    assert_eq!(body["max_tokens"], 256);
    assert_eq!(body["system"], "be brief");
    assert_eq!(body["messages"].as_array().unwrap().len(), 3);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["tools"][0]["name"], "calculator");
    assert_eq!(body["tool_choice"]["type"], "auto");
}

#[test]
fn anthropic_response_round_trip() {
    let raw = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "m",
        "content": [
            {"type": "text", "text": "the answer is 4"},
            {"type": "tool_use", "id": "toolu_9", "name": "calculator", "input": {"a": 2, "b": 2}},
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 3},
    });
    let parsed = anthropic::parse_response(&raw).unwrap();
    assert_eq!(parsed.stop_reason, Some(StopReason::ToolUse));
    assert_eq!(parsed.usage.cache_read_tokens, 3);
    assert_eq!(parsed.tool_uses().len(), 1);

    let back = anthropic::format_response(&parsed, "mock/mock-small");
    assert_eq!(back["stop_reason"], "tool_use");
    assert_eq!(back["usage"]["cache_read_input_tokens"], 3);
    assert_eq!(back["content"].as_array().unwrap().len(), 2);
}

// ----------------------------------------------------------- openai chat --

#[test]
fn openai_chat_request_parses() {
    let raw = serde_json::json!({
        "model": "mock/mock-small",
        "messages": [
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function",
                 "function": {"name": "calculator", "arguments": "{\"a\":2,\"b\":2}"}},
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": "4"},
        ],
        "tools": [{"type": "function", "function": {
            "name": "calculator", "description": "add", "parameters": {"type": "object"},
        }}],
        "tool_choice": "auto",
        "max_tokens": 64,
        "temperature": 0.5,
    });
    let req = openai_chat::parse_request(&raw).unwrap();
    assert_eq!(req.system.len(), 1);
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.max_tokens, Some(64));

    match &req.messages[1].content[0] {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "calculator");
            assert_eq!(input["a"], 2);
        }
        other => panic!("expected tool use, got {other:?}"),
    }
    match &req.messages[2].content[0] {
        ContentBlock::ToolResult { tool_use_id, .. } => assert_eq!(tool_use_id, "call_1"),
        other => panic!("expected tool result, got {other:?}"),
    }
}

#[test]
fn openai_chat_response_to_anthropic() {
    let raw = serde_json::json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "mock-small",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "the answer is 4",
                        "reasoning_content": "thinking"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 12, "completion_tokens": 7,
                  "prompt_tokens_details": {"cached_tokens": 4},
                  "completion_tokens_details": {"reasoning_tokens": 3}},
    });
    let parsed = openai_chat::parse_response(&raw).unwrap();
    assert_eq!(parsed.usage.input_tokens, 12);
    assert_eq!(parsed.usage.cache_read_tokens, 4);
    assert_eq!(parsed.usage.reasoning_tokens, 3);

    let back = anthropic::format_response(&parsed, "mock/mock-small");
    let content = back["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(back["stop_reason"], "end_turn");
}

#[test]
fn openai_chat_format_uses_tool_role() {
    let req = chat_request();
    let body = openai_chat::format_request(&req, "upstream-model");
    assert_eq!(body["model"], "upstream-model");
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tool_choice"], "auto");
}

// ------------------------------------------------------- openai responses --

#[test]
fn responses_request_parses() {
    let raw = serde_json::json!({
        "model": "mock/mock-small",
        "instructions": "be brief",
        "input": [
            {"role": "user", "content": [{"type": "input_text", "text": "hello"}]},
            {"type": "function_call", "call_id": "call_1", "name": "calculator", "arguments": "{\"a\":1}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "2"},
        ],
        "tools": [{"type": "function", "name": "calculator", "parameters": {"type": "object"}}],
        "reasoning": {"effort": "high"},
    });
    let req = openai_responses::parse_request(&raw).unwrap();
    assert_eq!(req.system.len(), 1);
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.reasoning_effort, Some(ReasoningEffort::High));
    match &req.messages[1].content[0] {
        ContentBlock::ToolUse { id, name, .. } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "calculator");
        }
        other => panic!("expected tool use, got {other:?}"),
    }
    match &req.messages[2].content[0] {
        ContentBlock::ToolResult { tool_use_id, .. } => assert_eq!(tool_use_id, "call_1"),
        other => panic!("expected tool result, got {other:?}"),
    }
}

#[test]
fn responses_response_formats() {
    let resp = ChatResponse {
        id: "resp_1".into(),
        model: "mock/mock-small".into(),
        content: vec![
            ContentBlock::text("hello "),
            ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "calculator".into(),
                input: serde_json::json!({"a": 1}),
            },
        ],
        stop_reason: Some(StopReason::ToolUse),
        usage: Usage::default(),
    };
    let out = openai_responses::format_response(&resp, "mock/mock-small");
    assert_eq!(out["object"], "response");
    assert_eq!(out["status"], "completed");
    let output = out["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "message");
    assert_eq!(output[1]["type"], "function_call");
    assert_eq!(output[1]["call_id"], "call_1");
}

// ----------------------------------------------------------------- gemini --

#[test]
fn gemini_request_parses() {
    let mut raw = serde_json::json!({
        "contents": [
            {"role": "user", "parts": [{"text": "hello"}]},
            {"role": "model", "parts": [
                {"functionCall": {"name": "calculator", "args": {"a": 1, "b": 2}}}
            ]},
            {"role": "user", "parts": [
                {"functionResponse": {"name": "calculator", "response": {"result": "3"}}}
            ]},
        ],
        "systemInstruction": {"parts": [{"text": "be brief"}]},
        "generationConfig": {"temperature": 0.2, "maxOutputTokens": 64},
        "tools": [{"functionDeclarations": [
            {"name": "calculator", "description": "add", "parameters": {"type": "object"}},
        ]}],
    });
    // The model arrives in the URL path, not the body; the gateway injects it.
    raw.as_object_mut()
        .unwrap()
        .insert("model".into(), serde_json::json!("mock/mock-small"));
    let req = gemini::parse_request(&raw).unwrap();
    assert_eq!(req.system.len(), 1);
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.max_tokens, Some(64));
    match &req.messages[1].content[0] {
        ContentBlock::ToolUse { name, input, .. } => {
            assert_eq!(name, "calculator");
            assert_eq!(input["a"], 1);
        }
        other => panic!("expected tool use, got {other:?}"),
    }
}

#[test]
fn gemini_response_formats() {
    let resp = ChatResponse {
        id: "gem-1".into(),
        model: "mock/mock-small".into(),
        content: vec![ContentBlock::text("the answer")],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage {
            input_tokens: 9,
            output_tokens: 4,
            ..Default::default()
        },
    };
    let out = gemini::format_response(&resp, "mock/mock-small");
    assert_eq!(out["candidates"][0]["finishReason"], "STOP");
    assert_eq!(
        out["candidates"][0]["content"]["parts"][0]["text"],
        "the answer"
    );
    assert_eq!(out["usageMetadata"]["promptTokenCount"], 9);
}

// --------------------------------------------------------------- streams --

/// Feed OpenAI chat chunks through the collector and render them as
/// Anthropic events; the frames should read as one Anthropic message.
#[test]
fn openai_stream_to_anthropic_stream() {
    let chunks = vec![
        serde_json::json!({"id": "c1", "model": "m", "choices": [
            {"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}]}),
        serde_json::json!({"id": "c1", "model": "m", "choices": [
            {"index": 0, "delta": {"content": "Hel"}, "finish_reason": null}]}),
        serde_json::json!({"id": "c1", "model": "m", "choices": [
            {"index": 0, "delta": {"content": "lo"}, "finish_reason": null}]}),
        serde_json::json!({"id": "c1", "model": "m", "choices": [
            {"index": 0, "delta": {"content": " there"}, "finish_reason": null}]}),
        serde_json::json!({"id": "c1", "model": "m", "choices": [
            {"index": 0, "delta": {}, "finish_reason": "stop"}]}),
        serde_json::json!({"id": "c1", "model": "m", "choices": [], "usage": {
            "prompt_tokens": 4, "completion_tokens": 3}}),
    ];

    let mut collector = openai_chat::Collector::new();
    let mut events = Vec::new();
    for c in &chunks {
        collector.push(c, &mut events);
    }
    collector.finish(&mut events);

    // One text block, one set of deltas, one close.
    let starts = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                StreamEvent::ContentBlockStart {
                    kind: BlockKind::Text,
                    ..
                }
            )
        })
        .count();
    assert_eq!(starts, 1);
    let stops = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ContentBlockStop { .. }))
        .count();
    assert_eq!(stops, 1);
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));

    // The usage arrives in the message delta.
    let delta = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::MessageDelta { usage, .. } => Some(*usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(delta.input_tokens, 4);
    assert_eq!(delta.output_tokens, 3);

    // Rendered back into Anthropic frames, the text should read as one block.
    let mut renderer = anthropic::Renderer::new("msg_x".into(), "mock/mock-small".into());
    let mut text = String::new();
    let mut block_starts = 0;
    for ev in &events {
        for frame in renderer.render(ev) {
            let data: serde_json::Value = serde_json::from_str(&frame.data).unwrap();
            match data["type"].as_str() {
                Some("content_block_start") => block_starts += 1,
                Some("content_block_delta") => {
                    text.push_str(data["delta"]["text"].as_str().unwrap_or(""))
                }
                _ => {}
            }
        }
    }
    assert_eq!(block_starts, 1);
    assert_eq!(text, "Hello there");
}

#[test]
fn anthropic_stream_collector_reads_events() {
    let frames = vec![
        serde_json::json!({"type": "message_start", "message": {
            "id": "msg_1", "model": "m", "usage": {"input_tokens": 7}}}),
        serde_json::json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""}}),
        serde_json::json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": "hi"}}),
        serde_json::json!({"type": "content_block_stop", "index": 0}),
        serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 2}}),
        serde_json::json!({"type": "message_stop"}),
    ];
    let mut collector = anthropic::Collector::new();
    let mut events = Vec::new();
    for f in &frames {
        collector.push(f, &mut events);
    }
    collector.finish(&mut events);
    assert!(matches!(
        events.first(),
        Some(StreamEvent::MessageStart { .. })
    ));
    let delta = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::MessageDelta { usage, .. } => Some(*usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(delta.input_tokens, 7);
    assert_eq!(delta.output_tokens, 2);
}

/// A tool call that streams in pieces must survive the round trip.
#[test]
fn streamed_tool_call_keeps_its_arguments() {
    let mut collector = openai_chat::Collector::new();
    let mut events = Vec::new();
    for chunk in [
        serde_json::json!({"id": "c", "model": "m", "choices": [{"index": 0,
            "delta": {"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
            "function": {"name": "calculator", "arguments": ""}}]}, "finish_reason": null}]}),
        serde_json::json!({"id": "c", "model": "m", "choices": [{"index": 0,
            "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{\"a\": 2"}}]},
            "finish_reason": null}]}),
        serde_json::json!({"id": "c", "model": "m", "choices": [{"index": 0,
            "delta": {"tool_calls": [{"index": 0, "function": {"arguments": ", \"b\": 3}"}}]},
            "finish_reason": null}]}),
        serde_json::json!({"id": "c", "model": "m", "choices": [{"index": 0,
            "delta": {}, "finish_reason": "tool_calls"}]}),
    ] {
        collector.push(&chunk, &mut events);
    }
    collector.finish(&mut events);

    let mut json = String::new();
    let mut name = String::new();
    let mut id = String::new();
    for ev in &events {
        if let StreamEvent::ToolInputDelta { partial_json, .. } = ev {
            json.push_str(partial_json);
        }
        if let StreamEvent::ToolUseStart { id: i, name: n, .. } = ev {
            id = i.clone();
            name = n.clone();
        }
    }
    assert_eq!(name, "calculator");
    assert_eq!(id, "call_1");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["a"], 2);
    assert_eq!(parsed["b"], 3);
}

// ------------------------------------------------------------------- sse --

#[test]
fn sse_frames_parse_across_line_endings() {
    let body = "data: {\"a\":1}\n\ndata: {\"b\":2}\r\n\r\ndata: [DONE]\n\n";
    let mut out = Vec::new();
    let mut buffer = String::from(body);
    while let Some(pos) = local_router::gateway::client::find_frame_end(&buffer) {
        let frame = buffer[..pos.0].to_string();
        buffer = buffer[pos.1..].to_string();
        if let Some(v) = local_router::gateway::client::parse_frame(&frame) {
            out.push(v);
        }
    }
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["a"], 1);
    assert_eq!(out[1]["b"], 2);
}

#[test]
fn sse_frames_render_and_parse_back() {
    let frame = sse::SseFrame::named(
        "message_start",
        serde_json::json!({"type": "message_start"}).to_string(),
    );
    let text = frame.encode();
    assert!(text.starts_with("event: message_start\n"));
    assert!(text.contains("data: {\"type\":\"message_start\"}"));
}

// -------------------------------------------------------------- routing --

#[test]
fn url_building_matches_the_protocol() {
    let u = local_router::gateway::client::url_for;
    assert_eq!(
        u("https://api.x.com/v1/", Protocol::OpenAiChat, None, false),
        "https://api.x.com/v1/chat/completions"
    );
    assert_eq!(
        u("https://api.x.com", Protocol::OpenAiChat, None, false),
        "https://api.x.com/v1/chat/completions"
    );
    assert_eq!(
        u(
            "https://api.anthropic.com",
            Protocol::Anthropic,
            None,
            false
        ),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        u(
            "https://api.anthropic.com/v1",
            Protocol::Anthropic,
            None,
            false
        ),
        "https://api.anthropic.com/v1/messages"
    );
    // A relay that only speaks Anthropic on /v1 must not grow a second one.
    assert_eq!(
        u(
            "https://relay.example.com/v1/",
            Protocol::Anthropic,
            None,
            false
        ),
        "https://relay.example.com/v1/messages"
    );
    assert_eq!(
        u("https://g.co/v1beta", Protocol::Gemini, Some("m1"), true),
        "https://g.co/v1beta/models/m1:streamGenerateContent"
    );
    assert_eq!(
        u("https://g.co", Protocol::Gemini, Some("m1"), false),
        "https://g.co/v1beta/models/m1:generateContent"
    );
}

#[test]
fn a_stream_becomes_one_reply_again() {
    use local_router::protocol::ir::{BlockKind, ChatResponse, StreamEvent};

    // A reply that is only reasoning still carries its thinking.
    let events = vec![
        StreamEvent::MessageStart {
            id: "r-1".into(),
            model: "m".into(),
            usage: Default::default(),
        },
        StreamEvent::ContentBlockStart {
            index: 0,
            kind: BlockKind::Thinking,
        },
        StreamEvent::ThinkingDelta {
            index: 0,
            text: "thinking".into(),
        },
        StreamEvent::ContentBlockStop { index: 0 },
        StreamEvent::MessageDelta {
            stop_reason: Some(local_router::protocol::ir::StopReason::MaxTokens),
            usage: Default::default(),
        },
        StreamEvent::MessageStop,
    ];
    let reply = ChatResponse::from_events(&events);
    assert_eq!(reply.content.len(), 1);
    assert_eq!(reply.text(), "");
    match &reply.content[0] {
        local_router::protocol::ir::ContentBlock::Thinking { text, .. } => {
            assert_eq!(text, "thinking")
        }
        other => panic!("expected a thinking block, got {other:?}"),
    }

    // Text and reasoning in the same reply both survive.
    let events = vec![
        StreamEvent::MessageStart {
            id: "r-2".into(),
            model: "m".into(),
            usage: Default::default(),
        },
        StreamEvent::ContentBlockStart {
            index: 0,
            kind: BlockKind::Thinking,
        },
        StreamEvent::ThinkingDelta {
            index: 0,
            text: "why ".into(),
        },
        StreamEvent::ContentBlockStop { index: 0 },
        StreamEvent::ContentBlockStart {
            index: 1,
            kind: BlockKind::Text,
        },
        StreamEvent::TextDelta {
            index: 1,
            text: "hello".into(),
        },
        StreamEvent::ContentBlockStop { index: 1 },
        StreamEvent::MessageDelta {
            stop_reason: Some(local_router::protocol::ir::StopReason::EndTurn),
            usage: Default::default(),
        },
        StreamEvent::MessageStop,
    ];
    let reply = ChatResponse::from_events(&events);
    assert_eq!(reply.text(), "hello");
    assert_eq!(reply.content.len(), 2);
    assert_eq!(
        reply.stop_reason,
        Some(local_router::protocol::ir::StopReason::EndTurn)
    );
}
