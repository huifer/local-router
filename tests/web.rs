//! The web console's guard, over HTTP.
//!
//! Loopback stays key-less; any other bind needs a key on `/api`; writes
//! from a mismatched Origin are refused wherever the page is served from.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use local_router::config::{AppState, Config, Settings, Store};

/// One more unique number per test, so concurrent stores never share a file.
static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn state_at(web_addr: &str) -> AppState {
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("local-router-web-test-{}-{n}", std::process::id()));
    let store = Arc::new(Store::at(dir.join(format!("config-{n}.json"))));
    let config = Config {
        settings: Settings {
            web_addr: web_addr.to_string(),
            ..Settings::default()
        },
        ..Config::default()
    };
    store.save(&config).expect("save test config");
    let state = AppState::load(store).await.expect("load test state");
    // Like the server does: the bind address wins over the file.
    state.set_web_bind(web_addr).await;
    state
}

fn console(state: AppState) -> Router {
    local_router::web::router(state)
}

async fn get(state: AppState, path: &str) -> (StatusCode, String) {
    let response = console(state)
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&body).to_string())
}

async fn post_with_origin(state: AppState, path: &str, origin: Option<&str>) -> StatusCode {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("host", "127.0.0.1:3426");
    if let Some(origin) = origin {
        builder = builder.header("origin", origin);
    }
    let response = console(state)
        .oneshot(builder.body(Body::from("{}")).unwrap())
        .await
        .unwrap();
    response.status()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_console_is_open_on_loopback() {
    let state = state_at("127.0.0.1").await;
    let (status, _) = get(state, "/api/status").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_console_needs_a_key() {
    let state = state_at("0.0.0.0").await;
    let (status, body) = get(state.clone(), "/api/status").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("Bearer"));

    let response = console(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .header("authorization", "Bearer local-router")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = console(state)
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .header("authorization", "Bearer nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_static_page_stays_open_without_a_key() {
    let state = state_at("0.0.0.0").await;
    let (status, _) = get(state, "/").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cross_origin_write_is_refused() {
    let state = state_at("127.0.0.1").await;
    let status = post_with_origin(state, "/api/groups", Some("http://evil.example")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_same_origin_write_passes_the_guard() {
    let state = state_at("127.0.0.1").await;
    // Same host:port as the Host header; the body is empty so the handler
    // rejects it on its own terms — anything but the guard's 403 is fine.
    let status = post_with_origin(state, "/api/groups", Some("http://127.0.0.1:3426")).await;
    assert_ne!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_without_an_origin_still_write() {
    let state = state_at("127.0.0.1").await;
    let status = post_with_origin(state, "/api/groups", None).await;
    assert_ne!(status, StatusCode::FORBIDDEN);
}

use std::sync::Arc;

// -------------------------------------------------------------- console api --
//
// The console's own data endpoints, over HTTP: the exact surface the React
// app consumes. Every assertion here mirrors a field `frontend/src/types.ts`
// declares, so a backend change that breaks the frontend turns red here.

use serde_json::Value;

async fn get_json(state: AppState, path: &str) -> (StatusCode, Value) {
    let response = console(state)
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap_or_default();
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_status_endpoint_carries_the_fields_the_header_needs() {
    let state = state_at("127.0.0.1").await;
    let (status, body) = get_json(state, "/api/status").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["version"].is_string());
    assert_eq!(body["providers"], 0);
    assert_eq!(body["groups"], 0);
    assert!(body["models"].is_number());
    let settings = &body["settings"];
    // The SettingsPanel reads every one of these.
    for field in [
        "gateway_port",
        "web_port",
        "lan",
        "gateway_addr",
        "web_addr",
        "default_caller_key",
        "send_agent_user_agent",
        "masking",
        "record_usage",
        "locale",
        "cors_origins",
        "workbuddy_auto_checkin",
    ] {
        assert!(
            settings.get(field).is_some(),
            "settings.{field} is missing; the console reads it"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_preset_list_includes_the_local_session_vendors() {
    let state = state_at("127.0.0.1").await;
    let (status, body) = get_json(state, "/api/presets").await;
    assert_eq!(status, StatusCode::OK);
    let groups = body["presets"].as_array().expect("presets array");
    let mut ids = Vec::new();
    let mut protocols = Vec::new();
    for group in groups {
        for preset in group["presets"].as_array().unwrap() {
            ids.push(preset["id"].as_str().unwrap().to_string());
            protocols.push(preset["protocol"].as_str().unwrap().to_string());
        }
    }
    assert!(ids.contains(&"workbuddy".to_string()));
    assert!(ids.contains(&"qwenwork".to_string()));
    // The qwenwork preset rides its own protocol, which the frontend type
    // must also know.
    assert!(protocols.contains(&"qwenwork".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_usage_endpoint_carries_every_panel_shape() {
    let state = state_at("127.0.0.1").await;
    let (status, body) = get_json(state, "/api/usage?period=7d").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["storage"].is_string());
    for field in [
        "totals",
        "by_model",
        "by_provider",
        "by_provider_costs",
        "by_agent",
        "by_key",
        "by_project",
        "by_session",
        "by_day",
        "requests_timeline",
        "context",
        "quota",
        "recent",
    ] {
        assert!(
            body.get(field).is_some(),
            "usage.{field} is missing; a Usage page panel reads it"
        );
    }
    let totals = &body["totals"];
    for field in [
        "requests",
        "failed",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "reasoning_tokens",
        "cost_usd",
        "credits",
        "unpriced",
    ] {
        assert!(totals.get(field).is_some(), "totals.{field} missing");
    }
    assert!(body["context"]["by_provider"].is_array());
    assert!(body["context"]["totals"].is_object());
    assert!(body["context"]["captured"].is_boolean());
}

#[tokio::test(flavor = "multi_thread")]
async fn projects_flow_through_the_console_api() {
    let dir = "/tmp/lr-web-proj-test";
    local_router::projects::note(dir);
    let state = state_at("127.0.0.1").await;

    // Listed with the row shape the Projects panel renders.
    let (status, body) = get_json(state.clone(), "/api/projects?period=7d").await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["projects"].as_array().expect("projects array");
    let row = rows
        .iter()
        .find(|r| r["id"] == dir)
        .expect("the noted project is listed");
    for field in [
        "id",
        "name",
        "color",
        "created_at",
        "budget_tokens",
        "budget_cost",
        "requests",
        "input_tokens",
        "output_tokens",
        "cost_usd",
        "credits",
        "last_seen",
    ] {
        assert!(row.get(field).is_some(), "project row.{field} missing");
    }

    // Renamed through the query-parameter endpoint (a directory cannot ride
    // in a path segment).
    let response = console(state.clone())
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/projects?id={}", urlencode(dir)))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Renamed"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (_, body) = get_json(state.clone(), "/api/projects?period=7d").await;
    let row = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == dir)
        .unwrap();
    assert_eq!(row["name"], "Renamed");

    // An unknown project answers 404, not a silent success.
    let response = console(state)
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/projects?id={}",
                    urlencode("/tmp/never-seen-xyz")
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Nope"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Percent-encode everything a path segment may not carry.
fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
