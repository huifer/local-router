//! Talking to QwenWork: the COSY-signed chat, the model list, and the
//! account-context.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use futures_util::StreamExt;
use serde_json::Value;

use crate::protocol::ir::{StreamEvent, Usage};
use crate::qwen::account::{parse_account_context, AccountQuota};
use crate::qwen::cosy;
use crate::qwen::login::{endpoint, ACCOUNT_CONTEXT_PATH, CHAT_PATH, MODELS_PATH, WALLETS_PATH};

/// One chat request QwenWork's Qoder-protocol agent endpoint understands.
/// This is the plaintext body (`agent_chat_generation` takes), before the
/// wire codec and the COSY envelope are applied.
#[derive(Debug, Clone, Default)]
pub struct QwenChatBody {
    pub model: String,
    pub messages: Vec<QwenMessage>,
    pub system: Vec<String>,
    pub tools: Vec<Value>,
    pub max_tokens: Option<u32>,
    pub enable_thinking: bool,
}

/// One turn, as QwenWork's chat takes it: text and image blocks.
#[derive(Debug, Clone)]
pub struct QwenMessage {
    pub role: String,
    pub content: Vec<QwenContent>,
}

#[derive(Debug, Clone)]
pub enum QwenContent {
    Text(String),
    Image(String),
}

/// The account's quota, read from the adapter account-context.
pub async fn account_quota(session: &crate::qwen::login::Session) -> anyhow::Result<AccountQuota> {
    let ctx = account_context(session).await?;
    let wallets = if wallet_supported() {
        wallets(session).await.ok()
    } else {
        None
    };
    Ok(parse_account_context(&ctx, wallets.as_ref()))
}

/// Whether the public cloud exposes wallets (the enterprise flagship does
/// not).
fn wallet_supported() -> bool {
    endpoint() == "https://gateway.qwenwork.cn"
}

async fn account_context(session: &crate::qwen::login::Session) -> anyhow::Result<Value> {
    let client = http()?;
    let headers = bearer(session);
    let response = client
        .get(format!("{}{}", endpoint(), ACCOUNT_CONTEXT_PATH))
        .headers(to_header_map(&headers)?)
        .send()
        .await
        .context("the QwenWork account-context could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the QwenWork account-context returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the QwenWork account-context was refused (HTTP {status}): {}",
            message_of(&body)
        );
    }
    Ok(body)
}

async fn wallets(session: &crate::qwen::login::Session) -> anyhow::Result<Value> {
    let client = http()?;
    let headers = bearer(session);
    let response = client
        .get(format!("{}{}", endpoint(), WALLETS_PATH))
        .headers(to_header_map(&headers)?)
        .send()
        .await
        .context("the QwenWork wallets could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the QwenWork wallets returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the QwenWork wallets were refused (HTTP {status}): {}",
            message_of(&body)
        );
    }
    Ok(body)
}

/// The model list the account may use (`qwork` scene).
pub async fn models(session: &crate::qwen::login::Session) -> anyhow::Result<Vec<String>> {
    let url = format!("{}{}", endpoint(), MODELS_PATH);
    let wire = "{}";
    let headers = signed_headers(&url, session, wire)?;
    let response = http()?
        .get(&url)
        .headers(to_header_map(&headers)?)
        .send()
        .await
        .context("the QwenWork model list could not reach the server")?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        anyhow::bail!("the QwenWork model list was refused (HTTP {status}): {text}");
    }
    let body: Value = serde_json::from_str(&text)
        .with_context(|| format!("the QwenWork model list was not json: {text}"))?;
    let mut out = Vec::new();
    if let Some(list) = body.get("qwork").and_then(Value::as_array) {
        for item in list {
            if let Some(key) = item.get("key").and_then(Value::as_str) {
                if !matches!(key, "" | "auto" | "default") {
                    out.push(key.to_string());
                }
            }
        }
    }
    Ok(out)
}

/// Build the COSY-signed headers for a QwenWork request.
fn signed_headers(
    url: &str,
    session: &crate::qwen::login::Session,
    wire: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    let rsa = cosy::rsa_key()?;
    let user = session.cosy_user();
    cosy::cosy_headers(url, &user, wire, &rsa, &session.machine_id)
}

/// A small HTTP client for QwenWork's API.
fn http() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("the QwenWork HTTP client could not be built")
}

fn bearer(session: &crate::qwen::login::Session) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert(
        "authorization".to_string(),
        format!("Bearer {}", session.device_token),
    );
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers
}

/// Run one chat request and stream the canonical events.
///
/// The request is built, encoded with the `Encode=1` codec, sent inside a
/// COSY envelope as SSE, and each QwenWork chunk is turned into
/// [`StreamEvent`]s (content, reasoning, OpenAI-native tool calls, usage).
pub async fn chat(
    session: &crate::qwen::login::Session,
    body: &QwenChatBody,
) -> anyhow::Result<Vec<StreamEvent>> {
    let model = model_config(&body.model);
    let plaintext = build_plaintext(body, &model);
    let wire = cosy::encode_body(&plaintext);
    let url = format!("{}{}", endpoint(), CHAT_PATH);
    let headers = signed_headers(&url, session, &wire)?;

    let client = http()?;
    let response = client
        .post(&url)
        .headers(to_header_map(&headers)?)
        .header("accept", "text/event-stream")
        .header("x-model-key", &body.model)
        .body(wire)
        .send()
        .await
        .context("the QwenWork chat request could not reach the server")?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("the QwenWork chat request was refused (HTTP {status}): {text}");
    }
    let bytes = response.bytes_stream();
    let mut events = Vec::new();
    parse_sse(bytes, &body.tools, &mut events).await;
    Ok(events)
}

/// A model's static config, as the catalog lists it.
pub fn model_config(model: &str) -> Value {
    serde_json::json!({
        "key": model,
        "name": model,
        "config": {},
    })
}

/// Build the plaintext chat body (`agent_chat_generation` request).
fn build_plaintext(body: &QwenChatBody, model: &Value) -> String {
    let system_text = if body.system.is_empty() {
        cosy::SYSTEM_PROMPT.to_string()
    } else {
        format!("{}\n\n{}", cosy::SYSTEM_PROMPT, body.system.join("\n\n"))
    };
    let sys = serde_json::json!({ "type": "text", "text": system_text });
    let messages: Vec<Value> =
        std::iter::once(serde_json::json!({ "role": "system", "content": [sys.clone()] }))
            .chain(body.messages.iter().map(|m| {
                let content: Vec<Value> = m
                    .content
                    .iter()
                    .map(|c| match c {
                        QwenContent::Text(t) => serde_json::json!({ "type": "text", "text": t }),
                        QwenContent::Image(url) => {
                            serde_json::json!({ "type": "image", "image_url": url })
                        }
                    })
                    .collect();
                serde_json::json!({ "role": m.role, "content": content })
            }))
            .collect();

    let params = serde_json::json!({
        "enable_thinking": body.enable_thinking,
        "max_tokens": body.max_tokens.unwrap_or(32000),
    });

    let mut request = serde_json::json!({
        "request_id": cosy::hex_id(),
        "request_set_id": cosy::hex_id(),
        "chat_record_id": cosy::hex_id(),
        "session_id": cosy::hex_id(),
        "parameters": params,
        "business": {
            "product": cosy::BUSINESS_PRODUCT,
            "version": cosy::COSY_VERSION,
            "type": "agent",
            "id": cosy::hex_id(),
            "name": "",
            "begin_at": now_ms(),
            "stage": "start",
        },
        "agent_id": "agent_common",
        "task_id": "common",
        "session_type": cosy::SESSION_TYPE,
        "model_config": model.get("config").cloned().unwrap_or(serde_json::json!({})),
        "system": [sys],
        "messages": messages,
    });
    if !body.tools.is_empty() {
        request["tools"] = serde_json::json!(body
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.get("function").and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or_default(),
                        "description": t.get("function").and_then(|f| f.get("description")).and_then(Value::as_str).unwrap_or_default(),
                        "parameters": t.get("function").and_then(|f| f.get("parameters")).cloned().unwrap_or(serde_json::json!({"type":"object","properties":{}})),
                    }
                })
            })
            .collect::<Vec<_>>());
    }
    serde_json::to_string(&request).unwrap_or_else(|_| "{}".to_string())
}

/// Read the SSE reply and turn each chunk into canonical events.
async fn parse_sse(
    mut stream: impl futures_util::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
    tools: &[Value],
    out: &mut Vec<StreamEvent>,
) {
    let mut buffer = String::new();
    let mut index: i32 = -1;
    let id = cosy::hex_id();
    let mut model = String::new();
    let mut saw_tool = false;
    let mut usage: Option<Usage> = None;

    out.push(StreamEvent::MessageStart {
        id: id.clone(),
        model: model.clone(),
        usage: Usage::default(),
    });

    while let Some(item) = stream.next().await {
        let Ok(bytes) = item else { break };
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = find_frame_end(&buffer) {
            let frame = buffer[..pos.0].to_string();
            buffer = buffer[pos.1..].to_string();
            if let Some(envelope) = parse_frame(&frame) {
                consume_envelope(
                    &envelope,
                    tools,
                    &mut index,
                    &mut model,
                    &mut saw_tool,
                    &mut usage,
                    out,
                );
            }
        }
    }
    if let Some(last) = parse_frame(&buffer) {
        consume_envelope(
            &last,
            tools,
            &mut index,
            &mut model,
            &mut saw_tool,
            &mut usage,
            out,
        );
    }
    // The usage rides the message's end, after every block has closed.
    out.push(StreamEvent::MessageDelta {
        stop_reason: Some(if saw_tool {
            crate::protocol::ir::StopReason::ToolUse
        } else {
            crate::protocol::ir::StopReason::EndTurn
        }),
        usage: usage.unwrap_or_default(),
    });
    out.push(StreamEvent::MessageStop);
}

/// One SSE frame → its JSON envelope payload.
fn parse_frame(frame: &str) -> Option<Value> {
    let mut data = String::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    serde_json::from_str::<Value>(data).ok()
}

fn find_frame_end(buffer: &str) -> Option<(usize, usize)> {
    let a = buffer.find("\n\n").map(|i| (i, i + 2));
    let b = buffer.find("\r\n\r\n").map(|i| (i, i + 4));
    match (a, b) {
        (Some(x), Some(y)) => Some(if x.0 <= y.0 { x } else { y }),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// Turn one QwenWork envelope into canonical events. Each `data:` line's
/// envelope has a `body` that is an OpenAI chunk.
fn consume_envelope(
    envelope: &Value,
    _tools: &[Value],
    index: &mut i32,
    model: &mut String,
    saw_tool: &mut bool,
    usage: &mut Option<Usage>,
    out: &mut Vec<StreamEvent>,
) {
    // A non-200 status in the envelope is a refusal.
    if let Some(code) = envelope.get("statusCodeValue").and_then(Value::as_i64) {
        if code != 200 {
            out.push(StreamEvent::Error {
                message: envelope
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("QwenWork refused the request")
                    .to_string(),
            });
            return;
        }
    }
    let inner: &str = match envelope.get("body").and_then(Value::as_str) {
        Some(s) => s,
        None => return,
    };
    if inner.trim() == "[DONE]" {
        return;
    }
    let Ok(chunk) = serde_json::from_str::<Value>(inner) else {
        return;
    };
    if model.is_empty() {
        if let Some(m) = chunk.get("model").and_then(Value::as_str) {
            *model = m.to_string();
        }
    }
    let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
        // A usage-only chunk carries no choices.
        if let Some(u) = chunk.get("usage") {
            *usage = Some(parse_usage(u));
        }
        return;
    };
    let Some(delta) = choices.first().and_then(|c| c.get("delta")) else {
        // An empty choices list with a usage object: the final count.
        if let Some(u) = chunk.get("usage") {
            *usage = Some(parse_usage(u));
        }
        return;
    };
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            *index += 1;
            out.push(StreamEvent::ContentBlockStart {
                index: *index as usize,
                kind: crate::protocol::ir::BlockKind::Text,
            });
            out.push(StreamEvent::TextDelta {
                index: *index as usize,
                text: text.to_string(),
            });
            out.push(StreamEvent::ContentBlockStop {
                index: *index as usize,
            });
        }
    }
    if let Some(text) = delta.get("reasoning_content").and_then(Value::as_str) {
        if !text.is_empty() {
            *index += 1;
            out.push(StreamEvent::ContentBlockStart {
                index: *index as usize,
                kind: crate::protocol::ir::BlockKind::Thinking,
            });
            out.push(StreamEvent::ThinkingDelta {
                index: *index as usize,
                text: text.to_string(),
            });
            out.push(StreamEvent::ContentBlockStop {
                index: *index as usize,
            });
        }
    }
    for tc in delta
        .get("tool_calls")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
    {
        let i = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let name = tc
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let args = tc
            .get("function")
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !name.is_empty() {
            *saw_tool = true;
            out.push(StreamEvent::ToolUseStart {
                index: i,
                id: tc
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_placeholder")
                    .to_string(),
                name,
            });
        }
        if !args.is_empty() {
            out.push(StreamEvent::ToolInputDelta {
                index: i,
                partial_json: args,
            });
        }
    }
    if let Some(u) = chunk.get("usage") {
        *usage = Some(parse_usage(u));
    }
}

fn parse_usage(u: &Value) -> Usage {
    Usage {
        input_tokens: u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: u
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens: u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_write_tokens: 0,
        reasoning_tokens: u
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

/// The message of an error payload, however the vendor wrapped it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("msg"))
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("the vendor refused the request")
        .to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

fn to_header_map(headers: &BTreeMap<String, String>) -> anyhow::Result<reqwest::header::HeaderMap> {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("{name} is not a valid header name"))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .with_context(|| format!("the value of {name} is not a valid header value"))?;
        map.insert(name, value);
    }
    Ok(map)
}
