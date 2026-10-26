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
