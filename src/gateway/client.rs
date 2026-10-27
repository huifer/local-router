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

/// The URL a request goes to.
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
        Protocol::QwenWork => {
            // Never used: the QwenWork module builds its own URL.
            let m = model.unwrap_or("flash");
            format!("{base}/qwenwork/{m}")
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

/// Build the path of an OpenAI-style auxiliary endpoint under a provider
/// base URL.
///
/// Providers normally configure either `https://host/v1` or `https://host`;
/// both forms should reach `/v1/images/...` and `/v1/videos/...`.
pub fn auxiliary_path(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = format!("/{}", path.trim_start_matches('/'));
    if base.ends_with("/v1") {
        path
    } else {
        format!("/v1{path}")
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
#[derive(Clone, Debug)]
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

/// The headers a request to a vendor carries.
pub fn auth_headers(
    protocol: Protocol,
    provider: &Provider,
    key: &str,
    agent_user_agent: Option<&str>,
    caller_headers: Option<&reqwest::header::HeaderMap>,
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
        Protocol::QwenWork => {
            // Never used: the QwenWork module signs its own headers.
            headers.insert("authorization".to_string(), format!("Bearer {key}"));
        }
    }
    for (name, value) in &provider.headers {
        // Static values may carry {version} / {platform} / {arch} — the way
        // codex-proxy's user_agent_template does.
        headers.insert(name.to_lowercase(), expand_template(value));
    }
    let ua = provider
        .user_agent
        .as_deref()
        .map(expand_template)
        .filter(|v| !v.is_empty())
        .or_else(|| agent_user_agent.map(|s| s.to_string()));
    match ua {
        Some(ua) if ua == "local-router" => {}
        Some(ua) => {
            headers.insert("user-agent".to_string(), ua);
        }
        None => {}
    }
    apply_header_rules(&mut headers, provider, caller_headers);
    headers
}

/// Expand `{version}` / `{platform}` / `{arch}` placeholders in a header
/// value, the way codex-proxy's `user_agent_template` does. Unknown
/// placeholders are left alone, so a header carrying JSON braces survives.
pub fn expand_template(value: &str) -> String {
    value
        .replace("{version}", env!("CARGO_PKG_VERSION"))
        .replace("{platform}", std::env::consts::OS)
        .replace("{arch}", std::env::consts::ARCH)
}

/// Apply the provider's header rules, in codex-proxy's order:
///
/// 1. `forward_headers` — copy the named headers from the caller's request
///    verbatim, when the caller sent them (Console Go refuses requests
///    without the client's own `x-opencode-session`; relays that log want
///    the caller's `x-request-id`).
/// 2. `remove_headers` — drop named headers last, so a rule here wins over
///    the auth header and anything injected above.
///
/// `header_order` is applied last, by `ordered_headers`, at send time.
fn apply_header_rules(
    headers: &mut BTreeMap<String, String>,
    provider: &Provider,
    agent_headers: Option<&reqwest::header::HeaderMap>,
) {
    if let Some(incoming) = agent_headers {
        for name in &provider.forward_headers {
            let name = name.to_lowercase();
            if let Some(value) = incoming
                .get(&name)
                .and_then(|v| v.to_str().ok())
                .filter(|v| !v.trim().is_empty())
            {
                headers.insert(name, value.to_string());
            }
        }
    }
    for name in &provider.remove_headers {
        headers.remove(&name.to_lowercase());
    }
}

/// Send-time shape of the header map: the provider's `header_order` when it
/// lists any, else the natural order. Ordering is a hint to picky relays;
/// HTTP itself does not care.
pub fn ordered_headers(
    mut headers: BTreeMap<String, String>,
    order: &[String],
) -> Vec<(String, String)> {
    if order.is_empty() {
        return headers.into_iter().collect();
    }
    let mut out: Vec<(String, String)> = Vec::with_capacity(headers.len());
    for name in order {
        let name = name.to_lowercase();
        if let Some(value) = headers.remove(&name) {
            out.push((name, value));
        }
    }
    out.extend(headers);
    out
}

/// The headers for a provider that authenticates with a local app's
/// session rather than a stored key.
pub fn session_headers(
    protocol: Protocol,
    provider: &Provider,
    session: &crate::workbuddy::Session,
) -> BTreeMap<String, String> {
    let mut headers = session.auth_headers();
    // A session provider speaks OpenAI's Chat shape, so the bearer token
    // goes in the same header it would for a stored key.
    headers.remove("x-api-key");
    headers.remove("x-goog-api-key");
    if protocol == Protocol::Anthropic {
        headers.remove("authorization");
        headers.insert("x-api-key".to_string(), session.access_token.clone());
    }
    for (name, value) in &provider.headers {
        headers.insert(name.to_lowercase(), value.clone());
    }
    apply_header_rules(&mut headers, provider, None);
    headers
}

/// A vendor's reply, either whole or streamed.
pub enum UpstreamReply {
    Whole { status: u16, body: Value },
    Stream { status: u16, bytes: ByteStream },
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
    let url = match &provider.chat_path {
        // A vendor that does not use the protocol's default path.
        Some(path) => format!("{}{}", base.trim_end_matches('/'), path),
        None => url_for(&base, target.protocol, Some(&target.model), stream),
    };

    let mut req = client.post(&url).json(&body);
    // The provider's header_order, when it lists any, shapes the wire.
    for (name, value) in ordered_headers(headers, &provider.header_order) {
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
    let really_streaming =
        stream && (200..300).contains(&status) && content_type.contains("event-stream");
    if really_streaming {
        let bytes: ByteStream = Box::pin(resp.bytes_stream());
        Ok(UpstreamReply::Stream { status, bytes })
    } else {
        let text = resp.text().await.unwrap_or_default();
        let parsed: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| serde_json::json!({"error": {"message": text}}));
        Ok(UpstreamReply::Whole {
            status,
            body: parsed,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------- url_for --\n
    #[test]
    fn url_for_builds_each_protocols_endpoint() {
        // OpenAI-style bases may already carry /v1.
        assert_eq!(
            url_for(
                "https://api.deepseek.com/v1",
                Protocol::OpenAiChat,
                None,
                false
            ),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            url_for("https://api.vendor.com", Protocol::OpenAiChat, None, false),
            "https://api.vendor.com/v1/chat/completions"
        );
        assert_eq!(
            url_for(
                "https://api.vendor.com/v1",
                Protocol::OpenAiResponses,
                None,
                false
            ),
            "https://api.vendor.com/v1/responses"
        );
        assert_eq!(
            url_for("https://api.vendor.com/", Protocol::Anthropic, None, false),
            "https://api.vendor.com/v1/messages"
        );
        // Trailing slashes are trimmed before joining.
        assert_eq!(
            url_for(
                "https://api.vendor.com//",
                Protocol::OpenAiChat,
                None,
                false
            ),
            "https://api.vendor.com//v1/chat/completions".replace("//v1", "/v1")
        );
    }

    #[test]
    fn url_for_gemini_switches_action_and_fills_the_model() {
        assert_eq!(
            url_for(
                "https://generativelanguage.googleapis.com/v1beta",
                Protocol::Gemini,
                Some("glm-4.6"),
                true
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/glm-4.6:streamGenerateContent"
        );
        assert_eq!(
            url_for("https://gemini.vendor.com", Protocol::Gemini, None, false),
            "https://gemini.vendor.com/v1beta/models/gemini-2.5-flash:generateContent"
        );
    }

    #[test]
    fn auxiliary_and_models_paths_follow_the_v1_family() {
        assert_eq!(
            auxiliary_path("https://api.vendor.com/v1", "/images/generations"),
            "/images/generations"
        );
        assert_eq!(
            auxiliary_path("https://api.vendor.com", "/images/generations"),
            "/v1/images/generations"
        );
        assert_eq!(
            auxiliary_path("https://api.vendor.com/", "images/edits"),
            "/v1/images/edits"
        );
        assert_eq!(
            models_url("https://api.vendor.com/v1"),
            "https://api.vendor.com/v1/models"
        );
        assert_eq!(
            models_url("https://api.vendor.com"),
            "https://api.vendor.com/v1/models"
        );
    }

    // -------------------------------------------------------- frames --\n
    #[test]
    fn frames_split_on_lf_lf_and_crlf_crlf() {
        assert_eq!(find_frame_end("data: {\"a\":1}\n\nrest"), Some((13, 15)));
        assert_eq!(
            find_frame_end("data: {\"a\":1}\r\n\r\nrest"),
            Some((13, 17))
        );
        // The earlier terminator wins when both are present.
        let both = "a\n\nb\r\n\r\n";
        let (end, next) = find_frame_end(both).unwrap();
        assert_eq!(&both[..end], "a");
        assert_eq!(&both[next..], "b\r\n\r\n");
        assert_eq!(find_frame_end("no terminator here"), None);
    }

    #[test]
    fn parse_frame_joins_data_lines_and_skips_sentinels() {
        assert_eq!(
            parse_frame("data: {\"x\":1}"),
            Some(serde_json::json!({"x": 1}))
        );
        // Multi-line data is joined with newlines (SSE semantics).
        let joined = parse_frame("data: {\"a\":\ndata: 1}");
        assert!(joined.is_some());
        // A sentinel and an empty frame produce nothing.
        assert_eq!(parse_frame("data: [DONE]"), None);
        assert_eq!(parse_frame("event: ping\ndata: "), None);
        // Broken JSON is dropped, not a panic.
        assert_eq!(parse_frame("data: {oops"), None);
        // CRLF line endings survive.
        assert_eq!(
            parse_frame("data: {\"y\":2}\r\n"),
            Some(serde_json::json!({"y": 2}))
        );
    }
}

#[cfg(test)]
mod header_rewrite_tests {
    use super::*;
    use reqwest::header::HeaderMap;

    fn provider(
        headers: &[(&str, &str)],
        forward: &[&str],
        remove: &[&str],
        order: &[&str],
    ) -> Provider {
        Provider {
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            forward_headers: forward.iter().map(|s| s.to_string()).collect(),
            remove_headers: remove.iter().map(|s| s.to_string()).collect(),
            header_order: order.iter().map(|s| s.to_string()).collect(),
            ..Provider::default()
        }
    }

    fn caller(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        map
    }

    // codex-proxy 的 user_agent_template 语义：占位符展开，未知占位符保留。
    #[test]
    fn templates_expand_version_platform_arch_and_keep_unknowns() {
        let out = expand_template("codex_cli_rs/{version} ({platform} {arch})");
        assert_eq!(
            out,
            format!(
                "codex_cli_rs/{} ({} {})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
        // 未知占位符原样保留（JSON 体的花括号不能被误伤）。
        assert_eq!(expand_template("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(expand_template("{nonce}"), "{nonce}");
    }

    // forward_headers：调用方的头原样带到上游；没带就不造一个。
    #[test]
    fn forward_headers_copy_the_callers_values_verbatim() {
        let p = provider(&[], &["x-opencode-session", "x-request-id"], &[], &[]);
        let incoming = caller(&[("x-opencode-session", "sess-42"), ("x-request-id", "req-9")]);
        let out = auth_headers(Protocol::OpenAiChat, &p, "sk", None, Some(&incoming));
        assert_eq!(
            out.get("x-opencode-session").map(String::as_str),
            Some("sess-42")
        );
        assert_eq!(out.get("x-request-id").map(String::as_str), Some("req-9"));
        // 没发送的转发头不会凭空出现。
        let empty = HeaderMap::new();
        let out = auth_headers(Protocol::OpenAiChat, &p, "sk", None, Some(&empty));
        assert!(!out.contains_key("x-opencode-session"));
    }

    // remove_headers 最后跑：能删掉默认鉴权头（例如中转站只要自己的头）。
    #[test]
    fn remove_headers_win_over_auth_and_static() {
        let p = provider(
            &[("x-static", "yes")],
            &[],
            &["authorization", "x-static"],
            &[],
        );
        let out = auth_headers(Protocol::OpenAiChat, &p, "sk", None, None);
        assert!(!out.contains_key("authorization"));
        assert!(!out.contains_key("x-static"));
        // 鉴权头还在的默认行为不受影响。
        let plain = Provider::default();
        let out = auth_headers(Protocol::OpenAiChat, &plain, "sk", None, None);
        assert_eq!(
            out.get("authorization").map(String::as_str),
            Some("Bearer sk")
        );
    }

    // 转发 + 删除叠加：转发进来的也可能被删掉。
    #[test]
    fn remove_runs_after_forward() {
        let p = provider(&[], &["x-session"], &["x-session"], &[]);
        let incoming = caller(&[("x-session", "s")]);
        let out = auth_headers(Protocol::OpenAiChat, &p, "sk", None, Some(&incoming));
        assert!(!out.contains_key("x-session"));
    }

    // header_order：列出的先按序排，其余跟在后面。
    #[test]
    fn header_order_puts_listed_headers_first() {
        let p = provider(&[], &[], &[], &["user-agent", "authorization"]);
        let out = auth_headers(Protocol::OpenAiChat, &p, "sk", Some("agent-ua"), None);
        let ordered = ordered_headers(
            out,
            &["user-agent".to_string(), "authorization".to_string()],
        );
        let names: Vec<&str> = ordered.iter().map(|(k, _)| k.as_str()).collect();
        let ua = names.iter().position(|n| *n == "user-agent").unwrap();
        let auth = names.iter().position(|n| *n == "authorization").unwrap();
        assert!(
            ua < auth,
            "user-agent should come before authorization: {names:?}"
        );
        // 没列的头跟在后面，不丢。
        assert!(!names.contains(&"anthropic-version")); // openai 协议没有这个头
        assert!(ordered.iter().any(|(k, _)| k == "authorization"));
    }

    // UA 模板作为静态头注入时同样展开。
    #[test]
    fn static_header_values_expand_placeholders() {
        let p = provider(
            &[("user-agent", "my-relay/{version} ({arch})")],
            &[],
            &[],
            &[],
        );
        let out = auth_headers(Protocol::OpenAiChat, &p, "sk", None, None);
        let ua = out.get("user-agent").unwrap();
        assert!(ua.contains(env!("CARGO_PKG_VERSION")), "ua {ua}");
        assert!(ua.contains(std::env::consts::ARCH), "ua {ua}");
        assert!(!ua.contains("{version}"));
    }
}
