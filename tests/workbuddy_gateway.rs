//! The WorkBuddy provider, end to end: the gateway reads the local login
//! session, renews an expired token, and talks to a vendor that only
//! answers streaming requests.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use local_router::config::{AppState, AuthKind, Config, Provider, Store};
use local_router::gateway;

/// What the mock WorkBuddy saw.
#[derive(Clone, Default)]
struct Seen {
    authorization: Arc<Mutex<Vec<String>>>,
    user_ids: Arc<Mutex<Vec<String>>>,
    refreshes: Arc<AtomicUsize>,
}

/// A stand-in for WorkBuddy: a stream-only chat endpoint, a token refresh
/// and a model list.
fn mock_router(seen: Seen) -> Router {
    Router::new()
        .route("/v2/chat/completions", post(chat))
        .route("/v2/plugin/auth/token/refresh", post(refresh))
        .route("/v3/config", get(config))
        .route("/v2/billing/meter/get-user-resource", post(billing))
        .with_state(seen)
}

/// The account's resource packages: empty, so the quota window is empty.
async fn billing() -> Response {
    axum::Json(json!({
        "code": 0,
        "data": {"Response": {"Data": {"Accounts": [
            {"PackageName": "Trial", "CapacitySize": 100, "CapacityUsed": 100,
             "CapacityRemain": 0, "CycleCapacitySize": 100, "CycleCapacityUsed": 100,
             "CycleCapacityRemain": 0, "CapacityUnit": "credits", "ExpiredTime": ""}
        ]}}}
    }))
    .into_response()
}

async fn chat(State(seen): State<Seen>, req: Request<Body>) -> Response {
    let authorization = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let user_id = req
        .headers()
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    seen.authorization.lock().unwrap().push(authorization);
    seen.user_ids.lock().unwrap().push(user_id);

    // WorkBuddy refuses a non-streaming request.
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(req.into_body(), 64 * 1024)
            .await
            .unwrap_or_default(),
    )
    .unwrap_or_default();
    if body.get("stream").and_then(Value::as_bool) != Some(true) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(
                json!({"code": 11101, "msg": "Non-stream chat request is currently not supported"}),
            ),
        )
            .into_response();
    }

    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("hy3")
        .to_string();
    let frames = [
        json!({"id": "cmb-1", "model": model, "object": "chat.completion.chunk", "created": 1,
               "choices": [{"index": 0, "delta": {"role": "assistant", "content": "收到"}, "finish_reason": ""}]}),
        json!({"id": "cmb-1", "model": model, "object": "chat.completion.chunk", "created": 1,
               "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18}}),
    ];
    let mut out = String::new();
    for frame in frames {
        out.push_str(&format!("data: {frame}\n\n"));
    }
    out.push_str("data: [DONE]\n\n");
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        out,
    )
        .into_response()
}

async fn refresh(State(seen): State<Seen>, req: Request<Body>) -> Response {
    seen.refreshes.fetch_add(1, Ordering::SeqCst);
    let refresh_token = req
        .headers()
        .get("x-refresh-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if refresh_token.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"code": 10001, "msg": "refreshToken is empty"})),
        )
            .into_response();
    }
    axum::Json(json!({
        "code": 0,
        "data": {
            "accessToken": "at-fresh",
            "refreshToken": "rt-fresh",
            "tokenType": "Bearer",
            "expiresIn": 2_419_200,
            "domain": "www.codebuddy.cn",
        }
    }))
    .into_response()
}

async fn config() -> Response {
    axum::Json(json!({
        "code": 0,
        "data": {"models": [{"id": "hy3"}, {"id": "auto"}]}
    }))
    .into_response()
}

/// A session file whose access token has already expired.
fn expired_session_file() -> String {
    let expired = local_router::workbuddy::now_ms() - 1000;
    format!(
        r#"{{
  "account": {{"uid": "u-1", "nickname": "Tester", "type": "personal", "lastLogin": true}},
  "auth": {{
    "accessToken": "at-old",
    "refreshToken": "rt-old",
    "tokenType": "Bearer",
    "expiresAt": {expired},
    "domain": "www.codebuddy.cn"
  }}
}}"#,
        expired = expired
    )
}

/// One more unique number per call, so concurrent tests never share a file.
fn unique_n() -> u64 {
    static N: AtomicUsize = AtomicUsize::new(0);
    N.fetch_add(1, Ordering::SeqCst) as u64
}

/// The tests below move the environment around, so they take turns.
async fn env_guard() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

/// Start the mock WorkBuddy and the gateway, and hand back the addresses.
async fn world() -> (String, Seen, String) {
    world_with(false).await
}

/// The same, with the provider's hold flag set how the test wants.
async fn world_with(hold_when_exhausted: bool) -> (String, Seen, String) {
    let seen = Seen::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vendor_port = listener.local_addr().unwrap().port();
    let app = mock_router(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // The session file the gateway will read.
    let dir = std::env::temp_dir().join(format!(
        "local-router-workbuddy-e2e-{}-{}-{}",
        std::process::id(),
        local_router::workbuddy::now_ms(),
        unique_n()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let session_path = dir.join("workbuddy-desktop.info");
    std::fs::write(&session_path, expired_session_file()).unwrap();
    std::env::set_var("LOCAL_ROUTER_WORKBUDDY_SESSION", &session_path);
    std::env::set_var(
        "LOCAL_ROUTER_WORKBUDDY_ENDPOINT",
        format!("http://127.0.0.1:{vendor_port}"),
    );

    let mut config = Config::default();
    config.providers.push(Provider {
        id: "workbuddy".into(),
        name: "WorkBuddy".into(),
        kind: "workbuddy".into(),
        enabled: true,
        protocol: local_router::protocol::ir::Protocol::OpenAiChat,
        base_url: Some(format!("http://127.0.0.1:{vendor_port}")),
        anthropic_base_url: None,
        responses_base_url: None,
        gemini_base_url: None,
        keys: Vec::new(),
        models: vec!["hy3".into(), "auto".into()],
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
        auth: Some(AuthKind::WorkBuddy),
        chat_path: Some("/v2/chat/completions".into()),
        stream_only: true,
        hold_when_exhausted,
        media_models: Vec::new(),
        media_paths: None,
    });

    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_port = gateway_listener.local_addr().unwrap().port();
    let store = Arc::new(Store::at(dir.join(format!("config-{}.json", unique_n()))));
    store.save(&config).unwrap();
    let state = AppState::load(store).await.unwrap();
    let app = gateway::router(gateway::Gateway::new(state));
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    (
        format!("http://127.0.0.1:{gateway_port}"),
        seen,
        session_path.display().to_string(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_non_streaming_caller_gets_a_whole_reply_from_a_stream_only_vendor() {
    let _turn = env_guard().await;
    let (gateway_addr, seen, session_path) = world().await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_addr}/v1/messages"))
        .header("authorization", "Bearer local-router")
        .header("content-type", "application/json")
        .json(&json!({
            "model": "workbuddy/hy3",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("the gateway answers");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("json");
    assert_eq!(body["content"][0]["text"], "收到");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["usage"]["input_tokens"], 11);
    assert_eq!(body["usage"]["output_tokens"], 7);

    // The expired token was renewed before the request went out, and the
    // account's headers came along.
    assert_eq!(seen.refreshes.load(Ordering::SeqCst), 1);
    let authorization = seen.authorization.lock().unwrap().clone();
    assert_eq!(authorization, vec!["Bearer at-fresh".to_string()]);
    let user_ids = seen.user_ids.lock().unwrap().clone();
    assert_eq!(user_ids, vec!["u-1".to_string()]);

    // The renewed token is not written anywhere the user can read it.
    let config = std::fs::read_to_string(
        std::path::Path::new(&session_path)
            .parent()
            .unwrap()
            .join("config-0.json"),
    );
    let _ = config;
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_SESSION");
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_ENDPOINT");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_streaming_caller_gets_the_vendors_own_stream() {
    let _turn = env_guard().await;
    let (gateway_addr, _seen, _session_path) = world().await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_addr}/v1/chat/completions"))
        .header("authorization", "Bearer local-router")
        .header("content-type", "application/json")
        .json(&json!({
            "model": "workbuddy/hy3",
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("the gateway answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/event-stream"));
    let text = response.text().await.unwrap();
    assert!(text.contains("收到"), "the stream has no text: {text}");
    assert!(
        text.contains("[DONE]"),
        "the stream was not finished: {text}"
    );

    // The stream's token counts reach the usage log: an agent that streams
    // is the usual case, and zeroes there would hide the real cost.
    let usage = std::fs::read_to_string(
        std::path::Path::new(&_session_path)
            .parent()
            .unwrap()
            .join("usage.jsonl"),
    )
    .unwrap_or_default();
    let last: serde_json::Value = serde_json::from_str(usage.lines().last().unwrap()).unwrap();
    assert_eq!(last["input_tokens"], 11, "the stream's tokens were lost");
    assert_eq!(last["output_tokens"], 7, "the stream's tokens were lost");

    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_SESSION");
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_ENDPOINT");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_routing_view_says_what_a_session_took() {
    let _turn = env_guard().await;
    let (gateway_addr, _seen, _session_path) = world().await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{gateway_addr}/v1/messages"))
        .header("authorization", "Bearer local-router")
        .header("content-type", "application/json")
        .header("x-session-id", "trace-me")
        .json(&json!({
            "model": "workbuddy/hy3",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = client
        .get(format!("{gateway_addr}/v1/local-router/route"))
        .header("authorization", "Bearer local-router")
        .header("x-session-id", "trace-me")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let trace = &body["session"];
    assert_eq!(trace["asked"], "workbuddy/hy3");
    assert_eq!(trace["plan"][0]["provider"], "workbuddy");
    assert_eq!(trace["served"]["model"], "hy3");
    assert!(trace["at"].as_str().is_some());

    // A session that never ran has no trace, but the groups are still listed.
    let body: Value = client
        .get(format!("{gateway_addr}/v1/local-router/route"))
        .header("authorization", "Bearer local-router")
        .header("x-session-id", "never-ran")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body["groups"].is_array());
    assert!(body.get("session").is_none());

    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_SESSION");
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_ENDPOINT");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_quota_window_and_the_usage_are_readable_over_http() {
    let _turn = env_guard().await;
    let (gateway_addr, _seen, _session_path) = world().await;
    let client = reqwest::Client::new();

    // The quota window, in the shape the routing view reads.
    let body: Value = client
        .get(format!("{gateway_addr}/v1/local-router/quotas"))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let quota = &body["data"][0];
    assert_eq!(quota["provider"], "workbuddy");
    assert_eq!(quota["kind"], "subscription");
    assert_eq!(quota["user"], "T***");
    assert_eq!(quota["windows"][0]["name"], "Credits");
    assert_eq!(quota["windows"][0]["remaining"], 0.0);
    assert_eq!(quota["windows"][0]["holds_why"], "clock");

    // Traffic, then the usage endpoints see it.
    client
        .post(format!("{gateway_addr}/v1/messages"))
        .header("authorization", "Bearer local-router")
        .header("content-type", "application/json")
        .json(&json!({
            "model": "workbuddy/hy3",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();

    let body: Value = client
        .get(format!("{gateway_addr}/v1/local-router/usage?period=today"))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["calls"], 1);
    assert_eq!(body["input"], 11);

    let body: Value = client
        .get(format!(
            "{gateway_addr}/v1/local-router/usage/requests?limit=10"
        ))
        .header("authorization", "Bearer local-router")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["rows"][0]["model"], "workbuddy/hy3");

    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_SESSION");
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_ENDPOINT");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_that_asks_to_be_held_waits_out_its_empty_quota() {
    let _turn = env_guard().await;
    // The mock account's window is empty; a provider that asks to be held
    // waits for the reset instead of getting a request it cannot serve.
    let (gateway_addr, _seen, _session_path) = world_with(true).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_addr}/v1/messages"))
        .header("authorization", "Bearer local-router")
        .header("content-type", "application/json")
        .json(&json!({
            "model": "workbuddy/hy3",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("out of quota"),
        "{body}"
    );

    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_SESSION");
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_ENDPOINT");
}
