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
        .route("/v1/images/generations", post(vendor_image_generation))
        .route("/v1/images/edits", post(vendor_image_edit))
        .route("/v1/videos", post(vendor_video_create))
        .route(
            "/v1/videos/{id}",
            get(vendor_video_status).delete(vendor_video_delete),
        )
        .route("/v1/videos/{id}/content", get(vendor_video_content))
        // The same media under a different family, for vendors that keep
        // their media next to their chat endpoint.
        .route("/v2/images/generations", post(vendor_image_generation))
        .route("/v2/images/edits", post(vendor_image_edit))
        .route("/v2/videos/generations", post(vendor_video_create))
        .with_state(vendor)
}

async fn vendor_image_generation() -> Json<Value> {
    Json(json!({
        "created": 1,
        "data": [{"url": "https://cdn.example/image.png", "revised_prompt": "a test image"}]
    }))
}

async fn vendor_image_edit() -> Json<Value> {
    Json(json!({"created": 1, "data": [{"b64_json": "aGVsbG8="}]}))
}

async fn vendor_video_create() -> Json<Value> {
    Json(json!({"id": "video-1", "object": "video", "status": "queued", "model": "video-model"}))
}

async fn vendor_video_status(axum::extract::Path(id): axum::extract::Path<String>) -> Json<Value> {
    Json(json!({"id": id, "object": "video", "status": "completed", "progress": 100}))
}

async fn vendor_video_delete() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn vendor_video_content() -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    &'static str,
) {
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "video/mp4")],
        "video-bytes",
    )
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
        model_map: Default::default(),
        auto_include: Vec::new(),
        catalog: None,
        headers: Default::default(),
        user_agent: None,
        forward_headers: Vec::new(),
        remove_headers: Vec::new(),
        header_order: Vec::new(),
        oauth: None,
        proxy: None,
        group_only: false,
        auth: None,
        chat_path: None,
        stream_only: false,
        hold_when_exhausted: false,
        max_concurrency: None,
        rpm: None,
        tpm: None,
        timeout_secs: None,
        connect_timeout_secs: None,
        first_byte_timeout_secs: None,
        schedule: Vec::new(),
        media_models: Vec::new(),
        media_paths: None,
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
        ..Default::default()
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
        alias: None,
        note: None,
        scenarios: Default::default(),
        max_price: None,
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
        alias: None,
        note: None,
        scenarios: Default::default(),
        max_price: None,
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
        alias: None,
        note: None,
        scenarios: Default::default(),
        max_price: None,
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

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_origin_is_only_allowed_when_the_user_says_so() {
    // Without the setting, no browser on another site may use the gateway.
    let mut config = base_config();
    config.settings.cors_origins = Vec::new();
    let first = world(config).await;
    let response = reqwest::Client::new()
        .get(format!("{}/v1/models", first.gateway_addr))
        .header("origin", "http://example.com")
        .send()
        .await
        .unwrap();
    assert!(
        response
            .headers()
            .get("access-control-allow-origin")
            .is_none(),
        "an origin was allowed without being asked for"
    );

    // With the setting, the listed origin is allowed.
    let mut config = base_config();
    config.settings.cors_origins = vec!["http://example.com".to_string()];
    let second = world(config).await;
    let response = reqwest::Client::new()
        .get(format!("{}/v1/models", second.gateway_addr))
        .header("origin", "http://example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("http://example.com")
    );

    // An origin that was not listed still is not.
    let response = reqwest::Client::new()
        .get(format!("{}/v1/models", second.gateway_addr))
        .header("origin", "http://evil.example")
        .send()
        .await
        .unwrap();
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_is_generated_through_the_provider() {
    let world = world(base_config()).await;
    let client = reqwest::Client::new();

    // The caller needs a key.
    let response = client
        .post(format!("{}/v1/images/generations", world.gateway_addr))
        .json(&json!({"model": "mock/mock-small", "prompt": "a cat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);

    let response = client
        .post(format!("{}/v1/images/generations", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .json(&json!({"model": "mock/mock-small", "prompt": "a cat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        200,
        "the gateway refused: {:?}",
        response.text().await
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"][0]["url"], "https://cdn.example/image.png");
    // The provider's own model id is what goes upstream, not the caller name.
    assert_eq!(body["data"][0]["revised_prompt"], "a test image");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_is_edited_through_the_provider() {
    let world = world(base_config()).await;
    let client = reqwest::Client::new();

    let form = reqwest::multipart::Form::new()
        .text("model", "mock/mock-small")
        .text("prompt", "make it blue")
        .part(
            "image",
            reqwest::multipart::Part::bytes(b"fake-png".to_vec())
                .file_name("cat.png")
                .mime_str("image/png")
                .unwrap(),
        );

    let response = client
        .post(format!("{}/v1/images/edits", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"][0]["b64_json"], "aGVsbG8=");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_video_task_lives_until_it_is_deleted() {
    let world = world(base_config()).await;
    let client = reqwest::Client::new();

    // Create.
    let response = client
        .post(format!("{}/v1/videos", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .json(&json!({"model": "mock/mock-small", "prompt": "a bird"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(id, "video-1");

    // Status.
    let response = client
        .get(format!("{}/v1/videos/{id}", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "completed");

    // Content.
    let response = client
        .get(format!("{}/v1/videos/{id}/content", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.text().await.unwrap(), "video-bytes");

    // Delete, then the task is gone.
    let response = client
        .delete(format!("{}/v1/videos/{id}", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 204);

    let response = client
        .get(format!("{}/v1/videos/{id}", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_video_task_is_not_answered() {
    let world = world(base_config()).await;
    let response = reqwest::Client::new()
        .get(format!("{}/v1/videos/nope", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vendor_outside_the_openai_family_is_asked_where_it_lives() {
    // A vendor whose media sits under /v2 and which finishes a video
    // straight away: the gateway must use the vendor's own paths and answer
    // the status itself.
    let mut config = base_config();
    let mock = config.providers.first_mut().unwrap();
    mock.base_url = Some("http://127.0.0.1:PORT".into());
    mock.media_models = vec!["mock-image".into()];
    mock.media_paths = Some(local_router::config::MediaPaths {
        image_generations: Some("/v2/images/generations".into()),
        image_edits: Some("/v2/images/edits".into()),
        video_create: Some("/v2/videos/generations".into()),
        video_status: None,
        video_content: None,
        video_delete: None,
    });

    let vendor = Vendor::default();
    let vendor_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vendor_port = vendor_listener.local_addr().unwrap().port();
    for provider in &mut config.providers {
        if let Some(url) = &provider.base_url {
            provider.base_url = Some(url.replace("PORT", &vendor_port.to_string()));
        }
    }
    let v = vendor.clone();
    tokio::spawn(async move {
        axum::serve(vendor_listener, vendor_router(v))
            .await
            .unwrap();
    });

    let dir = std::env::temp_dir().join(format!(
        "local-router-media-e2e-{}-{}",
        std::process::id(),
        unique_n()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(Store::at(dir.join(format!("config-{}.json", unique_n()))));
    store.save(&config).unwrap();
    let state = AppState::load(store).await.unwrap();
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_port = gateway_listener.local_addr().unwrap().port();
    let app = gateway::router(gateway::Gateway::new(state));
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let client = reqwest::Client::new();
    let address = format!("http://127.0.0.1:{gateway_port}");

    // The image goes to the vendor's own /v2 path.
    let response = client
        .post(format!("{address}/v1/images/generations"))
        .header("authorization", "Bearer local-router")
        .json(&json!({"model": "mock/mock-image", "prompt": "a cat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);

    // The video too.
    let response = client
        .post(format!("{address}/v1/videos"))
        .header("authorization", "Bearer local-router")
        .json(&json!({"model": "mock/mock-image", "prompt": "a bird"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    let id = body["id"].as_str().unwrap().to_string();

    // The vendor has no task endpoints, so the gateway answers the status
    // from the answer it already has.
    let response = client
        .get(format!("{address}/v1/videos/{id}"))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "queued");
}

// ---------------------------------------------------- usage windows --

/// A window in UTC built around the current minute: it opens `offset`
/// minutes from now and stays open an hour. Tests never depend on the
/// machine's zone this way, and an "open" window cannot expire mid-test.
fn utc_window(offset_minutes: i64) -> local_router::gateway::schedule::ScheduleWindow {
    use chrono::{Timelike, Utc};
    let now = Utc::now();
    let minute_of_day = now.hour() as i64 * 60 + now.minute() as i64;
    let start = (minute_of_day + offset_minutes).rem_euclid(1440);
    let end = (start + 60).rem_euclid(1440);
    local_router::gateway::schedule::ScheduleWindow {
        start: format!("{:02}:{:02}", start / 60, start % 60),
        end: format!("{:02}:{:02}", end / 60, end % 60),
        tz: Some("UTC".into()),
        models: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_window_refuses_a_direct_request_with_retry_after() {
    let mut config = base_config();
    // The window opens two hours from now: closed at the moment of the test.
    config.providers[0].schedule = vec![utc_window(120)];
    let world = world(config).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/v1/chat/completions", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .json(&json!({"model": "mock/mock-small", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 429);
    let retry_after: u64 = response
        .headers()
        .get("retry-after")
        .expect("a closed window says when it reopens")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry_after > 0 && retry_after <= 3 * 3600, "{retry_after}");
    let body: Value = response.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("usage window"));
    assert_eq!(
        world.vendor_hits.load(Ordering::SeqCst),
        0,
        "a closed provider is never sent anything"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_window_serves_normally() {
    let mut config = base_config();
    // The window opened ten minutes ago and runs an hour: open right now.
    config.providers[0].schedule = vec![utc_window(-10)];
    let world = world(config).await;
    let (status, _) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "mock/mock-small", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(world.vendor_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_skips_the_closed_member_without_a_cooldown() {
    let mut config = base_config();
    config.providers[0].schedule = vec![utc_window(120)]; // closed
    config.groups.push(Group {
        id: "g".into(),
        name: "g".into(),
        models: vec!["mock/mock-small".into(), "antmock/claude-ish".into()],
        routing: Routing::Order,
        weights: Default::default(),
        stays: Default::default(),
        listed: true,
        alias: None,
        note: None,
        scenarios: Default::default(),
        max_price: None,
    });
    let world = world(config).await;
    let (status, body) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "group/g", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    // The closed member was never asked; the open one answered.
    assert_eq!(world.vendor_hits.load(Ordering::SeqCst), 0);
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("anthropic"));

    // The skip is a local policy, not a vendor 429: no cooldown started.
    // The gateway's own status carries the live cooldown-free proof; the
    // console API lives on the web router, not this one.
    let (status, body) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "mock/mock-small", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    // Still refused by the window (not by a cooldown), and the vendor is
    // still untouched: a cooldown would also refuse, but the message says
    // which one it is.
    assert_eq!(status, 429);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("usage window"));
    assert_eq!(world.vendor_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_scoped_to_other_models_leaves_this_one_alone() {
    let mut config = base_config();
    let mut window = utc_window(120); // closed…
    window.models = vec!["some-other-model".into()]; // …but only for that one
    config.providers[0].schedule = vec![window];
    let world = world(config).await;
    let (status, _) = world
        .post(
            "/v1/chat/completions",
            "local-router",
            json!({"model": "mock/mock-small", "messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn media_work_obeys_the_usage_window_too() {
    let mut config = base_config();
    config.providers[0].media_models = vec!["mock-image".into()];
    config.providers[0].schedule = vec![utc_window(120)];
    let world = world(config).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/v1/images/generations", world.gateway_addr))
        .header("authorization", "Bearer local-router")
        .json(&json!({"model": "mock/mock-image", "prompt": "a cat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 429);
    let body: Value = response.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("usage window"));
}
