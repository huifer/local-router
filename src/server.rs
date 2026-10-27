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
}
