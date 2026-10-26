//! The WorkBuddy session: reading the login file, refreshing it, and the
//! credits the account holds.

use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::routing::post;
use axum::Router;
use serde_json::json;

use local_router::config::AuthKind;
use local_router::workbuddy;

/// A session file, written the way WorkBuddy writes it.
fn session_file(expires_at: i64) -> String {
    format!(
        r#"{{
  "account": {{
    "uid": "u-1",
    "nickname": "Tester",
    "type": "personal",
    "lastLogin": true
  }},
  "auth": {{
    "accessToken": "at-1",
    "refreshToken": "rt-1",
    "tokenType": "Bearer",
    "expiresAt": {expires_at},
    "refreshExpiresAt": {expires_at},
    "domain": "www.codebuddy.cn",
    "scope": "openid profile offline_access email"
  }}
}}"#,
        expires_at = expires_at
    )
}

/// The tests below move the environment around, so they take turns.
async fn env_guard() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

/// A temporary directory holding a session file.
fn with_session(expires_at: i64) -> (PathBuf, tempdir::TempDir) {
    let dir = tempdir::TempDir::new("local-router-workbuddy").expect("temp dir");
    let auth_dir = dir.path().join("auth");
    std::fs::create_dir_all(&auth_dir).expect("auth dir");
    let path = auth_dir.join("workbuddy-desktop.info");
    std::fs::write(&path, session_file(expires_at)).expect("session file");
    (path, dir)
}

/// A tiny stand-in for the `tempdir` crate, so the test needs no dependency.
mod tempdir {
    use std::path::{Path, PathBuf};

    /// A directory that deletes itself when it is dropped.
    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub fn new(prefix: &str) -> std::io::Result<Self> {
            let mut path = std::env::temp_dir();
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            path.push(format!("{prefix}-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&path)?;
            Ok(TempDir { path })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[tokio::test]
async fn the_session_file_is_parsed() {
    let _turn = env_guard().await;
    let expires = workbuddy::now_ms() + 86_400_000;
    let (_path, dir) = with_session(expires);
    std::env::set_var("WORKBUDDY_HOME", dir.path());
    let session = workbuddy::read_session_file().expect("the session file is read");
    assert_eq!(session.account.uid, "u-1");
    assert_eq!(session.account.nickname, "Tester");
    assert_eq!(session.access_token, "at-1");
    assert_eq!(session.refresh_token, "rt-1");
    assert_eq!(session.domain, "www.codebuddy.cn");
    std::env::remove_var("WORKBUDDY_HOME");
}

#[tokio::test]
async fn a_fresh_token_is_not_refreshed() {
    let _turn = env_guard().await;
    let expires = workbuddy::now_ms() + 86_400_000;
    let (_path, dir) = with_session(expires);
    std::env::set_var("WORKBUDDY_HOME", dir.path());
    let session = workbuddy::read_session_file().expect("the session file is read");
    assert!(!session.needs_refresh());
    std::env::remove_var("WORKBUDDY_HOME");
}

#[tokio::test]
async fn a_token_about_to_expire_is_refreshed() {
    let _turn = env_guard().await;
    let expires = workbuddy::now_ms() + 60_000;
    let (_path, dir) = with_session(expires);
    std::env::set_var("WORKBUDDY_HOME", dir.path());
    let session = workbuddy::read_session_file().expect("the session file is read");
    assert!(session.needs_refresh());
    std::env::remove_var("WORKBUDDY_HOME");
}

#[tokio::test]
async fn the_headers_carry_the_bearer_token_and_the_account() {
    let _turn = env_guard().await;
    let expires = workbuddy::now_ms() + 86_400_000;
    let (_path, dir) = with_session(expires);
    std::env::set_var("WORKBUDDY_HOME", dir.path());
    let session = workbuddy::read_session_file().expect("the session file is read");
    let headers = session.auth_headers();
    assert_eq!(headers.get("authorization").unwrap(), "Bearer at-1");
    assert_eq!(headers.get("x-user-id").unwrap(), "u-1");
    assert_eq!(headers.get("x-domain").unwrap(), "www.codebuddy.cn");
    assert_eq!(headers.get("x-product").unwrap(), "WorkBuddy");
    // The secret never appears under a name a log would print.
    assert!(!headers.keys().any(|k| k.contains("refresh")));
    std::env::remove_var("WORKBUDDY_HOME");
}

#[tokio::test]
async fn a_missing_session_file_is_reported_clearly() {
    let _turn = env_guard().await;
    std::env::set_var(
        "LOCAL_ROUTER_WORKBUDDY_SESSION",
        "/tmp/local-router-no-such-workbuddy-session.info",
    );
    let err = workbuddy::read_session_file().expect_err("there is no session file");
    let message = err.to_string();
    assert!(
        message.contains("sign in") || message.contains("not found"),
        "unhelpful message: {message}"
    );
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_SESSION");
}

#[tokio::test]
async fn the_workbuddy_provider_needs_no_key() {
    use local_router::config::Provider;
    let provider = Provider {
        id: "workbuddy".into(),
        name: "WorkBuddy".into(),
        kind: "workbuddy".into(),
        enabled: true,
        protocol: local_router::protocol::ir::Protocol::OpenAiChat,
        base_url: Some("https://copilot.tencent.com".into()),
        anthropic_base_url: None,
        responses_base_url: None,
        gemini_base_url: None,
        keys: Vec::new(),
        models: vec!["hy3".into()],
        auto_include: Vec::new(),
        catalog: None,
        headers: BTreeMap::new(),
        user_agent: None,
        proxy: None,
        group_only: false,
        auth: Some(AuthKind::WorkBuddy),
        chat_path: Some("/v2/chat/completions".into()),
        stream_only: true,
        hold_when_exhausted: false,
        media_models: Vec::new(),
        media_paths: None,
    };
    assert!(provider.uses_session());
    assert!(provider.enabled_keys().next().is_none());
    // The credential never lives in the config.
    let json = serde_json::to_string(&provider).expect("the provider is serialised");
    assert!(!json.contains("at-1"));
    assert!(json.contains("\"auth\":\"workbuddy\""));
}

/// A mock WorkBuddy whose check-in grants the first time and refuses after.
fn checkin_mock_router(granted_first: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Router {
    use axum::response::IntoResponse;

    async fn claim(
        axum::extract::State(granted_first): axum::extract::State<
            std::sync::Arc<std::sync::atomic::AtomicBool>,
        >,
    ) -> axum::response::Response {
        if granted_first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            axum::Json(json!({
                "code": 0,
                "msg": "OK",
                "data": {"daily_credit": 100, "streak_days": 8}
            }))
            .into_response()
        } else {
            (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(json!({"code": 10001, "msg": "今天已签到，请明天再来"})),
            )
                .into_response()
        }
    }

    Router::new()
        .route(
            "/v2/billing/meter/daily-checkin",
            post(claim).with_state(granted_first),
        )
        .route(
            "/v2/billing/meter/checkin-activity-status",
            post(|| async {
                axum::Json(json!({
                    "code": 0,
                    "data": {"active": true, "today_checked_in": false,
                             "streak_days": 7, "daily_credit": 100}
                }))
            }),
        )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daily_checkin_is_claimed_once_and_then_refused() {
    let _turn = env_guard().await;

    // A session whose token is still fresh.
    let expires = workbuddy::now_ms() + 86_400_000;
    let (_path, dir) = with_session(expires);
    std::env::set_var("WORKBUDDY_HOME", dir.path());

    let granted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, checkin_mock_router(granted))
            .await
            .unwrap();
    });
    std::env::set_var(
        "LOCAL_ROUTER_WORKBUDDY_ENDPOINT",
        format!("http://127.0.0.1:{port}"),
    );

    // The status says the day is still open.
    let status = workbuddy::checkin_state()
        .await
        .expect("the check-in state");
    assert!(status.active);
    assert!(!status.checked_in_today);
    assert_eq!(status.daily_credit, 100);

    // The first claim grants the credits.
    let outcome = workbuddy::claim_checkin().await.expect("the claim answers");
    assert!(outcome.granted, "{outcome:?}");
    assert_eq!(outcome.credit, Some(100));

    // The second claim of the day is refused, which is not an error.
    let outcome = workbuddy::claim_checkin().await.expect("the claim answers");
    assert!(!outcome.granted);
    assert!(outcome.already);
    assert!(outcome.message.contains("明天"), "{outcome:?}");

    std::env::remove_var("WORKBUDDY_HOME");
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_ENDPOINT");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_without_a_session_says_so() {
    let _turn = env_guard().await;
    std::env::set_var(
        "LOCAL_ROUTER_WORKBUDDY_SESSION",
        "/tmp/local-router-no-such-workbuddy-claim.info",
    );
    let err = workbuddy::claim_checkin()
        .await
        .expect_err("there is no session to claim with");
    assert!(err.to_string().contains("sign in"), "{err}");
    std::env::remove_var("LOCAL_ROUTER_WORKBUDDY_SESSION");
}
