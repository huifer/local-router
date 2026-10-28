//! The web console's guard, over HTTP.
//!
//! Loopback stays key-less; any other bind needs a key on `/api`; writes
//! from a mismatched Origin are refused wherever the page is served from.
//!
//! The env-locking test holds one std::sync::Mutex across awaits on
//! purpose: the whole test body must own the process-level env vars.
#![allow(clippy::await_holding_lock)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use local_router::config::{AppState, Config, ProviderKey, Settings, Store};

/// One more unique number per test, so concurrent stores never share a file.
static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The environment is process-global; tests that change it take this lock.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// The per-account quota and check-in endpoints act on one saved account,
/// and refuse the providers that have neither.
#[tokio::test(flavor = "multi_thread")]
async fn the_account_quota_and_checkin_are_per_account() {
    // The session pools live in the config dir: point it at a temp home so
    // this test never reads the user's own accounts.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = std::env::temp_dir().join(format!(
        "local-router-web-acct-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("LOCAL_ROUTER_HOME", &home);
    let state = state_at("127.0.0.1").await;

    // Trae CN has no quota of its own: the endpoint says so instead of
    // guessing.
    let (status, body) = get(
        state.clone(),
        "/api/providers/trae-cn/accounts/default/quota",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("trae-cn"), "{body}");

    // A provider that keeps no session has no per-account quota.
    let (status, body) = get(
        state.clone(),
        "/api/providers/openai/accounts/default/quota",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("per-account quota"), "{body}");

    // The daily check-in belongs to WorkBuddy only.
    let response = console(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/qwenwork/accounts/default/checkin")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // WorkBuddy itself: with no such account saved, the answer is a clear
    // error rather than a silent success.
    let response = console(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/providers/workbuddy/accounts/nobody/checkin")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    std::env::remove_var("LOCAL_ROUTER_HOME");
    let _ = std::fs::remove_dir_all(&home);
}

/// A stored key is readable on the machine the console runs on, so it can be
/// shown and copied — but never from another machine, even with the
/// gateway's own key.
#[tokio::test(flavor = "multi_thread")]
async fn a_stored_key_is_readable_on_loopback_only() {
    let state = state_at("127.0.0.1").await;
    let mut provider = local_router::config::Provider {
        id: "testkey".to_string(),
        name: "Test".to_string(),
        ..Default::default()
    };
    provider.keys = vec![ProviderKey {
        id: "k1".to_string(),
        key: "sk-secret-value-123456".to_string(),
        enabled: true,
        label: Some("primary".to_string()),
    }];
    let _ = state
        .update(|config| {
            config.providers.push(provider);
            Ok::<(), String>(())
        })
        .await;

    // On this machine the key comes back in full.
    let (status, body) = get(state.clone(), "/api/providers/testkey/keys/k1/secret").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("sk-secret-value-123456"), "{body}");

    // The provider list still masks it.
    let (status, body) = get(state.clone(), "/api/providers").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("sk-secret-value-123456"), "{body}");
    assert!(body.contains("sk-s…3456"), "{body}");

    // Away from the machine, the key stays out of reach even for a caller
    // that holds the gateway key.
    let remote = state_at("0.0.0.0").await;
    let mut remote_provider = local_router::config::Provider {
        id: "testkey".to_string(),
        name: "Test".to_string(),
        ..Default::default()
    };
    remote_provider.keys = vec![ProviderKey {
        id: "k1".to_string(),
        key: "sk-secret-value-123456".to_string(),
        enabled: true,
        label: Some("primary".to_string()),
    }];
    remote
        .update(|config| {
            config.providers.push(remote_provider);
            Ok::<(), String>(())
        })
        .await
        .ok();
    let response = console(remote)
        .oneshot(
            Request::builder()
                .uri("/api/providers/testkey/keys/k1/secret")
                .header("authorization", "Bearer local-router")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
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
    // The registry is process-global and resolves through the config dir:
    // point it at a temp home so this test never writes the user's file.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = std::env::temp_dir().join(format!(
        "local-router-web-proj-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("LOCAL_ROUTER_HOME", &home);
    local_router::projects::reload();
    let dir = home.join("work").to_string_lossy().to_string();
    local_router::projects::note(&dir);
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
                .uri(format!("/api/projects?id={}", urlencode(&dir)))
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
    std::env::remove_var("LOCAL_ROUTER_HOME");
    let _ = std::fs::remove_dir_all(&home);
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

// ------------------------------------------------- one port for both --
//
// The gateway and the console are one service on one port. These cases pin
// that: the page and the model endpoints answer on the same listener, and
// each keeps its own authentication rules.

use local_router::server;

async fn combined_state() -> AppState {
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "local-router-one-port-test-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(Store::at(dir.join(format!("config-{n}.json"))));
    let config = Config {
        settings: Settings {
            gateway_addr: "127.0.0.1".into(),
            ..Settings::default()
        },
        ..Config::default()
    };
    store.save(&config).expect("save test config");
    let state = AppState::load(store).await.expect("load test state");
    state.set_web_bind("127.0.0.1").await;
    state
}

#[tokio::test(flavor = "multi_thread")]
async fn the_console_and_the_model_api_answer_on_the_same_router() {
    let state = combined_state().await;
    let app = server::router(state);

    // The console's page, its assets and its API.
    for path in ["/", "/assets/app.js", "/assets/style.css", "/api/status"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path} should answer");
    }

    // The model endpoints are on the same router, and they still want a key.
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let models = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(models.status(), StatusCode::UNAUTHORIZED);

    let with_key = app
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .header("authorization", "Bearer local-router")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(with_key.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_console_guard_does_not_leak_onto_the_model_api() {
    let state = combined_state().await;
    let app = server::router(state);

    // A cross-origin write to the console is refused.
    let console = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/groups")
                .header("content-type", "application/json")
                .header("host", "127.0.0.1:3425")
                .header("origin", "http://evil.example")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(console.status(), StatusCode::FORBIDDEN);

    // The same origin header on a model endpoint is none of the console's
    // business: the request is judged by the gateway's own rules.
    let gateway = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("host", "127.0.0.1:3425")
                .header("origin", "http://evil.example")
                .body(Body::from(r#"{"model":"a/m","messages":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(gateway.status(), StatusCode::FORBIDDEN);
}
