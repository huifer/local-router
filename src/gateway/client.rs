//! Talking to the vendors.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

use crate::config::Provider;
use crate::protocol::ir::{Protocol, StreamEvent};

/// The bytes of a vendor's reply, as they arrive.
pub type ByteStream =
    Pin<Box<dyn futures_util::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

/// Reads a vendor's stream into the canonical event vocabulary.
pub trait StreamCollector: Send {
    /// Take one chunk of the vendor's stream and produce canonical events.
    fn push(&mut self, chunk: &Value, out: &mut Vec<StreamEvent>);
    /// Close the stream: flush whatever is left.
    fn finish(&mut self, out: &mut Vec<StreamEvent>);
}

/// The version local-router reports to vendors.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the URL a protocol's endpoint lives at.
pub fn url_for(base: &str, protocol: Protocol, model: Option<&str>, stream: bool) -> String {
    let base = base.trim_end_matches('/');
    match protocol {
        Protocol::OpenAiChat => {
            if base.ends_with("/v1") {
                format!("{base}/chat/completions")
            } else {
                format!("{base}/v1/chat/completions")
            }
        }
        Protocol::OpenAiResponses => {
            if base.ends_with("/v1") {
                format!("{base}/responses")
            } else {
                format!("{base}/v1/responses")
            }
        }
        Protocol::Anthropic => {
            if base.ends_with("/v1") {
                format!("{base}/messages")
            } else {
                format!("{base}/v1/messages")
            }
        }
        Protocol::Gemini => {
            let m = model.unwrap_or("gemini-2.5-flash");
            let action = if stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            };
            if base.ends_with("/v1beta") || base.ends_with("/v1") {
                format!("{base}/models/{m}:{action}")
            } else {
                format!("{base}/v1beta/models/{m}:{action}")
            }
        }
    }
}

/// The URL that lists a vendor's models.
pub fn models_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

/// One vendor, one key, one model: everything needed to make a call.
#[derive(Clone)]
pub struct Target {
    pub provider_id: String,
    pub provider_name: String,
    pub key_id: String,
    pub key_label: Option<String>,
    pub secret: String,
    /// The model id the vendor knows.
    pub model: String,
    /// The protocol the vendor speaks.
    pub protocol: Protocol,
    /// The full model id the caller asked for, e.g. `group/x` or `provider/model`.
    pub asked: String,
}

/// A prepared HTTP client for a provider.
pub fn build_client(provider: &Provider) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("local-router/{VERSION}"))
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(20));
    if let Some(proxy) = &provider.proxy {
        if !proxy.trim().is_empty() {
            let p = if proxy.starts_with("http") {
                proxy.clone()
            } else {
                format!("http://{proxy}")
            };
            let proxy = reqwest::Proxy::all(&p)?;
            builder = builder.proxy(proxy);
        }
    }
    Ok(builder.build()?)
}

/// The headers every request to a vendor carries.
pub fn auth_headers(
    protocol: Protocol,
    provider: &Provider,
    key: &str,
    agent_user_agent: Option<&str>,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    match protocol {
        Protocol::Anthropic => {
            headers.insert("x-api-key".to_string(), key.to_string());
            headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        }
        Protocol::Gemini => {
            headers.insert("x-goog-api-key".to_string(), key.to_string());
        }
        Protocol::OpenAiChat | Protocol::OpenAiResponses => {
            headers.insert("authorization".to_string(), format!("Bearer {key}"));
        }
    }
    for (name, value) in &provider.headers {
        headers.insert(name.to_lowercase(), value.clone());
    }
    let ua = provider
        .user_agent
        .as_deref()
        .filter(|v| !v.is_empty())
        .or(agent_user_agent);
    match ua {
        Some("local-router") | None => {}
        Some(ua) => {
            headers.insert("user-agent".to_string(), ua.to_string());
        }
    }
    headers
}

/// A vendor's reply, either whole or streamed.
pub enum UpstreamReply {
    Whole {
        status: u16,
        body: Value,
    },
    Stream {
        status: u16,
        bytes: ByteStream,
    },
}

/// Send a request to a vendor.
pub async fn send(
    client: &reqwest::Client,
    target: &Target,
    provider: &Provider,
    body: Value,
    headers: BTreeMap<String, String>,
    stream: bool,
) -> anyhow::Result<UpstreamReply> {
    let base = provider
        .base_url_for(target.protocol)
        .ok_or_else(|| anyhow::anyhow!("the provider {} has no url", target.provider_id))?;
    let url = url_for(&base, target.protocol, Some(&target.model), stream);

    let mut req = client.post(&url).json(&body);
    for (name, value) in &headers {
        req = req.header(name.as_str(), value.as_str());
    }
    if stream {
        req = req.header("accept", "text/event-stream");
    }

    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let really_streaming = stream
        && (200..300).contains(&status)
        && content_type.contains("event-stream");
    if really_streaming {
        let bytes: ByteStream = Box::pin(resp.bytes_stream());
        Ok(UpstreamReply::Stream { status, bytes })
    } else {
        let text = resp.text().await.unwrap_or_default();
        let parsed: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
            serde_json::json!({"error": {"message": text}})
        });
        Ok(UpstreamReply::Whole { status, body: parsed })
    }
}

/// Read an SSE body into chunks, whatever the vendor's framing.
pub async fn read_sse(mut stream: ByteStream) -> Vec<Value> {
    let mut buffer = String::new();
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        let Ok(bytes) = item else { break };
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        // Frames are separated by a blank line.
        while let Some(pos) = find_frame_end(&buffer) {
            let (frame, rest) = buffer.split_at(pos.0);
            let frame = frame.to_string();
            buffer = rest[pos.1..].to_string();
            if let Some(value) = parse_frame(&frame) {
                out.push(value);
            }
        }
    }
    if let Some(value) = parse_frame(&buffer) {
        out.push(value);
    }
    out
}

/// The byte range of the next complete frame, and where the rest starts.
pub fn find_frame_end(buffer: &str) -> Option<(usize, usize)> {
    let a = buffer.find("\n\n").map(|i| (i, i + 2));
    let b = buffer.find("\r\n\r\n").map(|i| (i, i + 4));
    match (a, b) {
        (Some(x), Some(y)) => Some(if x.0 <= y.0 { x } else { y }),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// Turn one SSE frame into its JSON payload.
pub fn parse_frame(frame: &str) -> Option<Value> {
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
