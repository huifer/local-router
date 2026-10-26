//! The whole gateway, over HTTP, against mock vendors.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use local_router::config::{AppState, Config, Group, Provider, ProviderKey, Routing, Store};
use local_router::gateway;

/// A port nothing is listening on, for the failover tests.
fn dead_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

/// A vendor that answers OpenAI chat requests, and can be told to fail.
#[derive(Clone, Default)]
struct Vendor {
    hits: Arc<AtomicUsize>,
    /// Answer with this status instead of 200 when set.
    fail_with: Arc<AtomicUsize>,
}

async fn vendor_chat(
    State(vendor): State<Vendor>,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    vendor.hits.fetch_add(1, Ordering::SeqCst);
    let fail = vendor.fail_with.load(Ordering::SeqCst);
    if fail != 0 {
        return (
            StatusCode::from_u16(fail as u16).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error": {"message": "the vendor refused"}})),
        );
    }
    let req: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    (
        StatusCode::OK,
        Json(json!({
            "id": "chatcmpl-e2e",
            "object": "chat.completion",
            "model": req["model"].as_str().unwrap_or("m"),
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "the mock says hi"},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 5, "total_tokens": 8},
        })),
    )
}

async fn vendor_models() -> Json<Value> {
    Json(json!({"object": "list", "data": [
        {"id": "mock-small", "object": "model"},
        {"id": "mock-big", "object": "model"},
    ]}))
}

fn vendor_router(vendor: Vendor) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(vendor_chat))
        .route("/v1/models", get(vendor_models))
        .with_state(vendor)
}

/// A second vendor that only speaks Anthropic.
async fn anthropic_vendor(body: axum::body::Bytes) -> (StatusCode, Json<Value>) {
    let req: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    (
        StatusCode::OK,
        Json(json!({
            "id": "msg_ant",
            "type": "message",
            "role": "assistant",
            "model": req["model"].as_str().unwrap_or("m"),
            "content": [{"type": "text", "text": "the anthropic mock says hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 4, "output_tokens": 6},
        })),
    )
}

fn provider(id: &str, port: u16, models: &[&str], keys: usize) -> Provider {
    Provider {
        id: id.to_string(),
        name: id.to_string(),
        kind: "custom".into(),
        enabled: true,
        protocol: local_router::protocol::ir::Protocol::OpenAiChat,
        base_url: Some(format!("http://127.0.0.1:{port}/v1")),
        anthropic_base_url: None,
        responses_base_url: None,
        gemini_base_url: None,
        keys: (0..keys)
            .map(|_| ProviderKey::new(format!("sk-{id}")))
            .collect(),
        models: models.iter().map(|m| m.to_string()).collect(),
        auto_include: Vec::new(),
        catalog: None,
        headers: Default::default(),
        user_agent: None,
        proxy: None,
        group_only: false,
    }
}

struct World {
    gateway_addr: String,
    vendor: Vendor,
    vendor_hits: Arc<AtomicUsize>,
}

/// Start both vendors and the gateway on free ports, with one configuration.
async fn world(config: Config) -> World {
    let vendor = Vendor::default();
    let vendor_hits = vendor.hits.clone();

    let vendor_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vendor_port = vendor_listener.local_addr().unwrap().port();
    let anthropic_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let anthropic_port = anthropic_listener.local_addr().unwrap().port();
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_port = gateway_listener.local_addr().unwrap().port();

    // Point the providers at the ports we actually bound.
    let mut config = config;
    for p in &mut config.providers {
        if let Some(url) = &p.base_url {
            if p.protocol == local_router::protocol::ir::Protocol::Anthropic {
                p.base_url = Some(url.replace("PORT", &anthropic_port.to_string()));
            } else {
                p.base_url = Some(url.replace("PORT", &vendor_port.to_string()));
            }
        }
    }

    let v = vendor.clone();
    tokio::spawn(async move {
        axum::serve(vendor_listener, vendor_router(v))
            .await
            .unwrap();
    });
    tokio::spawn(async move {
        let app = Router::new().route("/v1/messages", post(anthropic_vendor));
        axum::serve(anthropic_listener, app).await.unwrap();
    });

    let dir = std::env::temp_dir().join(format!(
        "local-router-e2e-{}-{}-{}",
        std::process::id(),
        now_millis(),
        unique_n()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(Store::at(dir.join(format!("config-{}.json", unique_n()))));
    store.save(&config).unwrap();
    let state = AppState::load(store).await.unwrap();

    let app = gateway::router(gateway::Gateway::new(state));
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    World {
        gateway_addr: format!("http://127.0.0.1:{gateway_port}"),
        vendor,
        vendor_hits,
    }
}

/// One more unique number per call, so concurrent tests never share a dir.
fn unique_n() -> u64 {
    static N: AtomicUsize = AtomicUsize::new(0);
    N.fetch_add(1, Ordering::SeqCst) as u64
}

fn now_millis() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl World {
    async fn post(&self, path: &str, key: &str, body: Value) -> (u16, Value) {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}{}", self.gateway_addr, path))
            .header("authorization", format!("Bearer {key}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let value: Value = resp.json().await.unwrap_or(Value::Null);
        (status, value)
    }

    async fn stream(&self, path: &str, key: &str, body: Value) -> String {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}{}", self.gateway_addr, path))
            .header("authorization", format!("Bearer {key}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        let mut stream = resp.bytes_stream();
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            out.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        }
        out
    }
}

fn base_config() -> Config {
    let openai = Provider {
        base_url: Some("http://127.0.0.1:PORT/v1".into()),
        ..provider("mock", 0, &["mock-small"], 1)
    };
    let mut anthropic = provider("antmock", 0, &["claude-ish"], 1);
    anthropic.protocol = local_router::protocol::ir::Protocol::Anthropic;
    anthropic.base_url = Some("http://127.0.0.1:PORT/v1".into());
    Config {
        providers: vec![openai, anthropic],
        groups: vec![],
        settings: Default::default(),
        caller_keys: vec![],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_gateway_serves_the_model_list() {
    let world = world(base_config()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/v1/models", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"mock/mock-small"));
    assert!(ids.contains(&"antmock/claude-ish"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_without_a_key_is_refused() {
    let world = world(base_config()).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/chat/completions", world.gateway_addr))
        .json(&json!({"model": "mock/mock-small", "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_with_the_wrong_key_is_refused() {
    let world = world(base_config()).await;
    let (status, _) = world
        .post(
            "/v1/chat/completions",
            "not-the-key",
            json!({"model": "mock/mock-small", "messages": []}),
        )
        .await;
    assert_eq!(status, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_openai_call_answers_in_openai_shape() {
    let world = world(base_config()).await;
    let (status, body) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "mock/mock-small", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["choices"][0]["message"]["content"], "the mock says hi");
    assert_eq!(body["model"], "mock/mock-small");
    assert_eq!(body["usage"]["prompt_tokens"], 3);
}

/// An Anthropic-shaped call reaches an OpenAI-shaped vendor and comes back
/// in Anthropic shape.
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_in_openai_out_translates() {
    let world = world(base_config()).await;
    let (status, body) = world
        .post(
            "/v1/messages",
            "local-router",
            json!({
                "model": "mock/mock-small",
                "max_tokens": 64,
                "system": "be brief",
                "messages": [{"role": "user", "content": "hi"}],
            }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["content"][0]["text"], "the mock says hi");
    assert_eq!(body["usage"]["input_tokens"], 3);
}

/// An OpenAI-shaped call reaches an Anthropic-shaped vendor and comes back
/// in OpenAI shape.
#[tokio::test(flavor = "multi_thread")]
async fn openai_in_anthropic_upstream_translates() {
    let world = world(base_config()).await;
    let (status, body) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "antmock/claude-ish", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "the anthropic mock says hi"
    );
    assert_eq!(body["usage"]["prompt_tokens"], 4);
    assert_eq!(body["usage"]["completion_tokens"], 6);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_fails_over_to_the_next_member() {
    let mut config = base_config();
    config
        .providers
        .push(provider("dead", dead_port(), &["ghost"], 1));
    config.groups.push(Group {
        id: "g".into(),
        name: "g".into(),
        models: vec!["dead/ghost".into(), "mock/mock-small".into()],
        routing: Routing::Order,
        weights: Default::default(),
        stays: Default::default(),
        listed: true,
        note: None,
    });
    let world = world(config).await;
    let (status, body) = world
        .post(
            "/v1/messages",
            "local-router",
            json!({"model": "group/g", "max_tokens": 32, "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["content"][0]["text"], "the mock says hi");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_of_dead_members_reports_the_error() {
    let mut config = base_config();
    config
        .providers
        .push(provider("dead", dead_port(), &["ghost"], 1));
    config.groups.push(Group {
        id: "g".into(),
        name: "g".into(),
        models: vec!["dead/ghost".into()],
        routing: Routing::Order,
        weights: Default::default(),
        stays: Default::default(),
        listed: true,
        note: None,
    });
    let world = world(config).await;
    let (status, body) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "group/g", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 502);
    assert!(!body["error"]["message"].as_str().unwrap_or("").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn streaming_anthropic_out_of_an_openai_vendor() {
    let world = world(base_config()).await;
    let text = world
        .stream(
            "/v1/messages",
            "local-router",
            json!({
                "model": "mock/mock-small",
                "stream": true,
                "max_tokens": 32,
                "messages": [{"role": "user", "content": "hi"}],
            }),
        )
        .await;
    assert!(text.contains("event: message_start"));
    assert!(text.contains("event: content_block_start"));
    assert!(text.contains("\"text\":\"the mock says hi\""));
    assert!(text.contains("event: message_stop"));
    let starts = text.matches("event: content_block_start").count();
    assert_eq!(starts, 1, "one text block, not one per chunk");
}

#[tokio::test(flavor = "multi_thread")]
async fn streaming_openai_out_of_an_anthropic_vendor() {
    let world = world(base_config()).await;
    let text = world
        .stream(
            "/v1/chat/completions",
            "local-router",
            json!({
                "model": "antmock/claude-ish",
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}],
            }),
        )
        .await;
    assert!(text.contains("\"content\":\"the anthropic mock says hi\""));
    assert!(text.contains("\"finish_reason\":\"stop\""));
    assert!(text.contains("[DONE]"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limited_key_reports_the_error() {
    let mut config = base_config();
    config.groups.push(Group {
        id: "g".into(),
        name: "g".into(),
        models: vec!["mock/mock-small".into()],
        routing: Routing::Order,
        weights: Default::default(),
        stays: Default::default(),
        listed: true,
        note: None,
    });
    let world = world(config).await;
    world.vendor.fail_with.store(429, Ordering::SeqCst);
    let (status, body) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "group/g", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 429);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("refused"));
}

#[tokio::test(flavor = "multi_thread")]
async fn two_keys_are_tried_in_turn() {
    let mut config = base_config();
    config.providers[0].keys = vec![ProviderKey::new("sk-first"), ProviderKey::new("sk-second")];
    let world = world(config).await;
    world.vendor.fail_with.store(500, Ordering::SeqCst);
    let _ = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "mock/mock-small", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    let hits = world.vendor_hits.load(Ordering::SeqCst);
    assert_eq!(hits, 2, "both keys should have been tried");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_body_is_a_bad_request() {
    let world = world(base_config()).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/chat/completions", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .header("content-type", "application/json")
        .body("this is not json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_model_is_not_found() {
    let world = world(base_config()).await;
    let (status, body) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "mock/ghost", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 404);
    assert!(body["error"]["message"].as_str().unwrap().contains("ghost"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_health_endpoint_tells_the_version() {
    let world = world(base_config()).await;
    let client = reqwest::Client::new();
    let body: Value = client
        .get(format!("{}/health", world.gateway_addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["version"].as_str().is_some());
}
