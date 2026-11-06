//! One HTTP server for the gateway and the console.
//!
//! The gateway and the console are two faces of the same local service, so
//! they share one listener and one port: the page, its assets and the
//! `/api` it talks to live next to the model endpoints. One port means one
//! address to remember, one origin for the browser (no CORS dance between
//! them), and one place to bind when the service is exposed to the network.
//!
//! Routing is by path, not by port:
//!
//! - the gateway's own endpoints (`/v1/*`, `/v1beta/*`, `/health`) are
//!   matched first and keep their per-request key check;
//! - everything else falls through to the console, which serves the page,
//!   its assets, the `/api` it talks to, and its own 404;
//! - the console's guard (loopback exemption, Origin check on writes) only
//!   ever sees console requests, because it lives inside the console's
//!   router.

use axum::Router;

use crate::config::AppState;
use crate::gateway::{self, Gateway};

/// The combined router: the gateway's API, with the console behind it.
pub fn router(state: AppState) -> Router {
    let gateway = Gateway::new(state.clone());
    // The gateway's routes win; the console answers everything else.
    gateway::router(gateway).fallback_service(crate::web::router(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn state() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "local-router-server-unit-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(crate::config::Store::at(dir.join("config.json")));
        let config = crate::config::Config::default();
        store.save(&config).unwrap();
        let state = AppState::load(store).await.unwrap();
        state.set_web_bind("127.0.0.1").await;
        state
    }

    #[tokio::test]
    async fn the_console_and_the_gateway_share_one_router() {
        let state = state().await;
        let app = router(state);

        // The console's page and its API are served.
        let page = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);

        let api = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api.status(), StatusCode::OK);

        // The gateway's own endpoints are there too, and they still want a
        // key: the console's loopback exemption does not leak to them.
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

        let health = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_endpoint_serves_prometheus_on_loopback() {
        crate::metrics::install();
        let state = state().await;
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("local_router_in_flight"),
            "missing in_flight gauge in /metrics output:\n{text}"
        );
    }

    use crate::config::{Config, Provider, ProviderKey};

    /// A state that already holds one provider carrying a real key.
    async fn state_with_provider() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "local-router-cfg-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(crate::config::Store::at(dir.join("config.json")));
        let mut config = Config::default();
        config.providers.push(Provider {
            id: "mock".into(),
            name: "mock".into(),
            base_url: Some("http://127.0.0.1:9".into()),
            keys: vec![ProviderKey::new("TOPSECRET")],
            ..Provider::default()
        });
        store.save(&config).unwrap();
        let state = AppState::load(store).await.unwrap();
        state.set_web_bind("127.0.0.1").await;
        state
    }

    #[tokio::test]
    async fn config_export_masks_keys_by_default() {
        let state = state_with_provider().await;
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let cfg: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let key = cfg["providers"][0]["keys"][0]["key"].as_str().unwrap();
        assert_eq!(key, "", "keys must be redacted on a default export");
        assert_eq!(cfg["revision"].as_u64(), Some(0));
        assert_eq!(cfg["schema_version"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn config_export_includes_keys_when_asked() {
        let state = state_with_provider().await;
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/config?include_keys=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let cfg: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let key = cfg["providers"][0]["keys"][0]["key"].as_str().unwrap();
        assert_eq!(key, "TOPSECRET");
    }

    #[tokio::test]
    async fn config_import_round_trip_bumps_revision() {
        let state = state_with_provider().await;
        let app = router(state);
        let exported = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/config?include_keys=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(exported.into_body(), usize::MAX)
            .await
            .unwrap();
        let cfg: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(cfg["revision"].as_u64(), Some(0));
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&cfg).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let out = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let out: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(out["revision"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn config_import_rejects_stale_revision() {
        let state = state_with_provider().await;
        let app = router(state);
        let exported = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/config?include_keys=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(exported.into_body(), usize::MAX)
            .await
            .unwrap();
        let cfg: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // First import succeeds and bumps the revision.
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&cfg).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        // Reusing the same (now stale) revision 0 loses.
        let second = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&cfg).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn config_import_rejects_duplicate_provider_id() {
        let state = state_with_provider().await;
        let app = router(state);
        let exported = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/config?include_keys=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(exported.into_body(), usize::MAX)
            .await
            .unwrap();
        let mut cfg: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Duplicate the first provider under a second entry with the same id.
        let first = cfg["providers"][0].clone();
        cfg["providers"].as_array_mut().unwrap().push(first);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&cfg).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
