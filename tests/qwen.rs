//! The QwenWork (千问办公) module: the body codec, the COSY envelope, the
//! account-context parsing, the device-flow login against a mock vendor,
//! and the gateway path that routes a qwenwork provider through it.
//!
//! The env-locking tests hold one std::sync::Mutex across awaits on
//! purpose: the whole test body must own the process-level env vars.
#![allow(clippy::await_holding_lock)]

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use local_router::config::{AppState, Config, Provider, Store};
use local_router::qwen::cosy;
use local_router::qwen::login::{LoginPoll, Session, SessionFile};

/// The process-level env vars (`LOCAL_ROUTER_HOME`, `LOCAL_ROUTER_QWEN_ENDPOINT`)
/// are shared by every test in this binary: anything that touches them holds
/// this lock for its whole body. Holding it across awaits is the point — the
/// whole test body must own the env, so the lint is silenced here.
#[allow(clippy::await_holding_lock)]
mod env_guard {
    use std::sync::Mutex;
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());
}

/// A temp dir with an isolated LOCAL_ROUTER_HOME, unique per test.
fn temp_home(tag: &str) -> (tempdir::TempDir, std::path::PathBuf) {
    let dir = tempdir::TempDir::new().expect("temp dir");
    let home = dir.path().join(tag);
    std::fs::create_dir_all(&home).unwrap();
    (dir, home)
}

/// Write a session file into the home and return the session it holds.
fn store_session(home: &std::path::Path, token: &str, refresh: &str, expires_in: i64) -> Session {
    let file = SessionFile {
        account: local_router::qwen::login::QwenAccount {
            uid: "u-1".into(),
            nickname: "Alice".into(),
            email: "alice@example.com".into(),
        },
        auth: local_router::qwen::login::QwenAuth {
            device_token: token.into(),
            refresh_token: refresh.into(),
            expires_at: Some(local_router::qwen::login::now_ms() + expires_in * 1000),
        },
        machine_id: "machine-1".into(),
    };
    let path = home.join("qwen-session.json");
    std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
    }
    local_router::qwen::login::read_session_file().unwrap()
}

// ---------------------------------------------------------------- codec ----

#[test]
fn codec_round_trip() {
    for sample in [
        "",
        "hello",
        "QwenWork 千问办公: 你好, 世界!",
        "{\"json\":true,\"n\":42,\"s\":\"with spaces and 中文\"}",
    ] {
        let wire = cosy::encode_body(sample);
        // The wire form never carries the plaintext and only uses the
        // shuffled alphabet plus the '$' pad.
        for c in wire.chars() {
            assert!(
                c == '$'
                    || "_doRTgHZBKcGVjlvpC,@aFSx#DPuNJme&i*MzLOEn)sUrthbf%Y^w.(kIQyXqWA!"
                        .contains(c),
                "unexpected codec char {c}"
            );
        }
        let decoded = cosy::decode_body(&wire).unwrap();
        assert_eq!(decoded, sample);
    }
}

#[test]
fn encode_body_hides_the_plaintext() {
    let wire = cosy::encode_body("{\"secret\":\"the-model-name\"}");
    assert!(!wire.contains("secret"));
    assert!(!wire.contains("model"));
}

// -------------------------------------------------------------- envelope ---

#[test]
fn cosy_headers_carry_the_qwenwork_identity() {
    let rsa = cosy::rsa_key().unwrap();
    let user = cosy::CosyUser {
        uid: "u-1",
        name: "Alice",
        email: "alice@example.com",
        access: "device-token-1",
    };
    let headers = cosy::cosy_headers(
        "https://gateway.qwenwork.cn/algo/api/v2/model/list?Encode=1",
        &user,
        "{}",
        &rsa,
        "machine-1",
    )
    .unwrap();

    // The site's own constants, which are what tell QwenWork apart from
    // its sibling Qoder sites.
    assert_eq!(headers.get("Cosy-ClientType").unwrap(), "6");
    assert_eq!(headers.get("Cosy-Scene").unwrap(), "qwork");
    assert_eq!(headers.get("Cosy-Business-Product").unwrap(), "qoder_work");
    assert_eq!(headers.get("Cosy-Version").unwrap(), "1.1.59");
    assert_eq!(headers.get("Cosy-User").unwrap(), "u-1");
    assert_eq!(headers.get("Cosy-MachineId").unwrap(), "machine-1");

    // The Authorization header is the COSY envelope: Bearer COSY.payload.sig
    let auth = headers.get("Authorization").unwrap();
    let rest = auth.strip_prefix("Bearer COSY.").expect("COSY envelope");
    let (payload, sig) = rest.rsplit_once('.').expect("payload.sig");
    assert!(!payload.is_empty());
    assert_eq!(sig.len(), 32); // md5, hex

    // The payload decodes to the envelope's inner json and names the blob.
    let decoded = base64_decode(payload);
    let inner: Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(inner["version"], "v1");
    assert_eq!(inner["cosyVersion"], "1.1.59");
    assert!(!inner["info"].as_str().unwrap().is_empty());

    // The Cosy-Key is the RSA-wrapped AES key: it never repeats.
    let other = cosy::cosy_headers(
        "https://gateway.qwenwork.cn/x",
        &user,
        "{}",
        &rsa,
        "machine-1",
    )
    .unwrap();
    assert_ne!(
        headers.get("Cosy-Key").unwrap(),
        other.get("Cosy-Key").unwrap()
    );
}

#[test]
fn cosy_headers_refuse_an_account_without_a_machine_id() {
    let rsa = cosy::rsa_key().unwrap();
    let user = cosy::CosyUser {
        uid: "u-1",
        name: "Alice",
        email: "alice@example.com",
        access: "t",
    };
    assert!(cosy::cosy_headers("https://x/y", &user, "{}", &rsa, "").is_err());
}

#[test]
fn the_signature_covers_the_wire_body_and_path() {
    // Two calls with the same account at the same second differ by body:
    // the md5 signature covers the exact wire form.
    let rsa = cosy::rsa_key().unwrap();
    let user = cosy::CosyUser {
        uid: "u",
        name: "",
        email: "",
        access: "t",
    };
    let h1 =
        cosy::cosy_headers("https://x/algo/api/v2/model/list", &user, "AAA", &rsa, "m").unwrap();
    let h2 =
        cosy::cosy_headers("https://x/algo/api/v2/model/list", &user, "BBB", &rsa, "m").unwrap();
    assert_ne!(
        h1.get("Authorization").unwrap(),
        h2.get("Authorization").unwrap()
    );
}

// --------------------------------------------------------------- account ---

#[test]
fn account_context_personal_reads_the_balance_and_pools() {
    let ctx = json!({
        "data": {
            "plan": {"name": "QwenWork 个人版", "next_refresh_date": "2026-11-01 00:00:00"},
            "quota": {"total": null, "used": null, "remaining": 980}
        }
    });
    let wallets = json!({
        "data": {
            "daily_credits": {"total_balance": 100.5},
            "monthly_credits": {"total_balance": 800},
            "longterm_credits": {"total_balance": 0}
        }
    });
    let q = local_router::qwen::account::parse_account_context(&ctx, Some(&wallets));
    assert_eq!(q.plan, "QwenWork 个人版");
    assert!(q.balance.starts_with("980"), "balance was {}", q.balance);
    assert!(q.balance.contains("daily 100.5"));
    assert!(q.balance.contains("monthly 800"));
    assert!(!q.empty);
    // The pool list carries the two non-empty wallets.
    assert_eq!(q.pools.len(), 2);
}

#[test]
fn account_context_with_a_total_keeps_the_used_window() {
    let ctx = json!({
        "plan": {"name": "Pro"},
        "quota": {"total": 1000, "used": 200, "remaining": 800}
    });
    let q = local_router::qwen::account::parse_account_context(&ctx, None);
    assert!(q.display.contains("200 / 1000"));
    assert!(!q.empty);
}

#[test]
fn account_context_exhausted_is_empty() {
    let ctx = json!({"quota": {"remaining": 0}});
    let q = local_router::qwen::account::parse_account_context(&ctx, None);
    assert!(q.empty);
}

// ----------------------------------------------------------------- login ---

#[tokio::test]
async fn session_file_round_trip_and_missing_file() {
    let _env = env_guard::ENV_LOCK.lock().unwrap();
    let (_dir, home) = temp_home("qwen-session-1");
    std::env::set_var("LOCAL_ROUTER_HOME", &home);

    // No session yet: a clear error, not a panic.
    let err = local_router::qwen::login::read_session_file().unwrap_err();
    assert!(err.to_string().contains("login"), "{err}");

    let session = store_session(&home, "tok-1", "ref-1", 3600);
    assert_eq!(session.device_token, "tok-1");
    assert_eq!(session.account.nickname, "Alice");
    assert!(!session.needs_refresh());

    // An about-to-expire token asks for a refresh.
    let soon = store_session(&home, "tok-2", "ref-2", 60);
    assert!(soon.needs_refresh());

    // A file without a token is refused.
    std::fs::write(
        home.join("qwen-session.json"),
        serde_json::to_string(&SessionFile::default()).unwrap(),
    )
    .unwrap();
    assert!(local_router::qwen::login::read_session_file().is_err());
}

/// Several QwenWork accounts live side by side: more than two, each with its
/// own identity and token, and the one in use can be switched.
#[tokio::test]
async fn several_qwenwork_accounts_live_side_by_side() {
    let _env = env_guard::ENV_LOCK.lock().unwrap();
    let (_dir, home) = temp_home("qwen-session-pool");
    std::env::set_var("LOCAL_ROUTER_HOME", &home);

    // Sign in three accounts, each filed under its own name.
    for (slot, uid) in [("default", "u-1"), ("work", "u-2"), ("personal", "u-3")] {
        let token = local_router::qwen::login::DeviceToken {
            token: format!("tok-{uid}"),
            refresh_token: format!("ref-{uid}"),
            expires_at: Some(local_router::qwen::login::now_ms() + 3_600_000),
            user_id: uid.to_string(),
        };
        let account = json!({"data": {"user": {
            "id": uid, "name": format!("User {uid}"), "email": format!("{uid}@example.com")
        }}});
        local_router::qwen::login::store_session(&token, &account, "machine-1", slot)
            .await
            .unwrap();
    }

    // All three are held; the last one signed in is the one in use.
    let pool = local_router::qwen::login::read_pool().unwrap();
    assert_eq!(pool.accounts.len(), 3);
    assert_eq!(pool.active, "personal");
    assert_eq!(pool.accounts["default"].account.uid, "u-1");
    assert_eq!(pool.accounts["work"].account.uid, "u-2");
    assert_eq!(pool.accounts["personal"].account.uid, "u-3");
    // Each account keeps its own token — they do not overwrite each other.
    assert_eq!(pool.accounts["default"].auth.device_token, "tok-u-1");
    assert_eq!(pool.accounts["work"].auth.device_token, "tok-u-2");
    assert_eq!(pool.accounts["personal"].auth.device_token, "tok-u-3");

    // The accounts list shows all of them.
    assert_eq!(local_router::qwen::login::accounts().unwrap().len(), 3);

    // A named account can be read on its own.
    let work = local_router::qwen::login::session_of("work").unwrap();
    assert_eq!(work.device_token, "tok-u-2");
    assert_eq!(work.account.nickname, "User u-2");

    // Switching the account in use works, and an unknown name is refused.
    local_router::qwen::login::use_account("work").unwrap();
    let pool = local_router::qwen::login::read_pool().unwrap();
    assert_eq!(pool.active, "work");
    assert_eq!(
        local_router::qwen::login::read_session_file()
            .unwrap()
            .device_token,
        "tok-u-2"
    );
    assert!(local_router::qwen::login::use_account("nope").is_err());

    // The rotating reader walks the accounts instead of always using one.
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..6 {
        let (name, _) = local_router::qwen::login::session_rotating().await.unwrap();
        seen.insert(name);
    }
    assert_eq!(seen.len(), 3, "every account should take a turn: {seen:?}");

    // An old single-session file still reads (as one account named default).
    std::fs::write(
        home.join("qwen-session.json"),
        serde_json::to_string(&SessionFile {
            account: local_router::qwen::login::QwenAccount {
                uid: "u-old".into(),
                nickname: "Old".into(),
                email: "old@example.com".into(),
            },
            auth: local_router::qwen::login::QwenAuth {
                device_token: "tok-old".into(),
                refresh_token: "ref-old".into(),
                expires_at: Some(local_router::qwen::login::now_ms() + 3_600_000),
            },
            machine_id: "machine-old".into(),
        })
        .unwrap(),
    )
    .unwrap();
    let pool = local_router::qwen::login::read_pool().unwrap();
    assert_eq!(pool.accounts.len(), 1);
    assert_eq!(pool.active, "default");
    assert_eq!(pool.accounts["default"].account.uid, "u-old");
}

/// Every sign-in is filed under its own slot, so any number of accounts can
/// live side by side. Signing the same person in twice gives two entries:
/// the browser's sign-in page signs in whoever is logged in there, and that
/// is the user's call — the pool never merges accounts behind their back.
#[tokio::test]
async fn every_sign_in_becomes_its_own_account() {
    let _env = env_guard::ENV_LOCK.lock().unwrap();
    let (_dir, home) = temp_home("qwen-session-multi");
    std::env::set_var("LOCAL_ROUTER_HOME", &home);

    let token_of = |tok: &str| local_router::qwen::login::DeviceToken {
        token: tok.to_string(),
        refresh_token: format!("ref-{tok}"),
        expires_at: Some(local_router::qwen::login::now_ms() + 3_600_000),
        user_id: "u-1".to_string(),
    };
    let account = json!({"data": {"user": {
        "id": "u-1", "name": "Zhang", "email": "z@example.com"
    }}});

    // The first sign-in lands in the slot it was given.
    let first = local_router::qwen::login::store_session(
        &token_of("tok-1"),
        &account,
        "machine-1",
        "default",
    )
    .await
    .unwrap();
    assert_eq!(first.slot, "default");
    assert!(!first.same_uid_elsewhere);

    // The second sign-in of the same person is kept as its own entry — it
    // is not merged into the first one.
    let second = local_router::qwen::login::store_session(
        &token_of("tok-2"),
        &account,
        "machine-1",
        "qwenwork-2",
    )
    .await
    .unwrap();
    assert_eq!(second.slot, "qwenwork-2");
    assert!(
        second.same_uid_elsewhere,
        "the pool says out loud that the uid is already there"
    );

    let pool = local_router::qwen::login::read_pool().unwrap();
    assert_eq!(pool.accounts.len(), 2);
    assert_eq!(pool.active, "qwenwork-2");
    assert_eq!(pool.accounts["default"].auth.device_token, "tok-1");
    assert_eq!(pool.accounts["qwenwork-2"].auth.device_token, "tok-2");

    // A different uid is a different account, and says nothing about it.
    let other = json!({"data": {"user": {
        "id": "u-2", "name": "Li", "email": "li@example.com"
    }}});
    let third = local_router::qwen::login::store_session(
        &token_of("tok-3"),
        &other,
        "machine-1",
        "qwenwork-3",
    )
    .await
    .unwrap();
    assert_eq!(third.slot, "qwenwork-3");
    assert!(!third.same_uid_elsewhere);
    assert_eq!(
        local_router::qwen::login::read_pool()
            .unwrap()
            .accounts
            .len(),
        3
    );

    // Removing an account drops it and moves the active marker when needed:
    // the account in use goes, the first remaining one takes over.
    let was_active = local_router::qwen::login::remove_account("qwenwork-3").unwrap();
    assert!(was_active);
    let pool = local_router::qwen::login::read_pool().unwrap();
    assert_eq!(pool.accounts.len(), 2);
    assert_eq!(pool.active, "default");
    assert!(local_router::qwen::login::remove_account("nope").is_err());
}

/// A stand-in QwenWork: the device flow's select-accounts, poll, refresh,
/// the account-context and the wallets.
async fn mock_qwen(
    seen_polls: Arc<AtomicUsize>,
    _seen_select: Arc<AtomicUsize>,
    poll_answer: Arc<Mutex<Value>>,
) -> std::net::SocketAddr {
    use axum::extract::State;
    use axum::http::{Request, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::Router;

    async fn poll(
        State((n, answer)): State<(Arc<AtomicUsize>, Arc<Mutex<Value>>)>,
        req: Request<axum::body::Body>,
    ) -> Response {
        n.fetch_add(1, Ordering::SeqCst);
        // The verifier and the nonce ride the query string.
        let q = req.uri().query().unwrap_or_default().to_string();
        assert!(q.contains("verifier="), "poll must send the verifier: {q}");
        assert!(q.contains("nonce="), "poll must send the nonce: {q}");
        let body = answer.lock().unwrap().clone();
        (StatusCode::OK, axum::Json(body)).into_response()
    }

    async fn refresh(req: Request<axum::body::Body>) -> Response {
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(req.into_body(), 64 * 1024)
                .await
                .unwrap_or_default(),
        )
        .unwrap_or_default();
        if body["refresh_token"].as_str().unwrap_or("").is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"message": "no refresh token"})),
            )
                .into_response();
        }
        axum::Json(json!({
            "token": "tok-new",
            "refresh_token": "ref-new",
            "expires_in": 3600
        }))
        .into_response()
    }

    async fn account_context(req: Request<axum::body::Body>) -> Response {
        let auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            auth.starts_with("Bearer "),
            "account-context needs a bearer: {auth}"
        );
        axum::Json(json!({
            "data": {
                "user": {"id": "u-9", "name": "Bob", "email": "bob@example.com"},
                "plan": {"name": "QwenWork"},
                "quota": {"total": null, "used": null, "remaining": 500}
            }
        }))
        .into_response()
    }

    async fn wallets() -> Response {
        axum::Json(json!({"data": {"daily_credits": {"total_balance": 300}}})).into_response()
    }

    let app = Router::new()
        .route("/api/v1/deviceToken/poll", get(poll))
        .route("/api/v1/deviceToken/refresh", post(refresh))
        .route("/api/v1/adapter/user/account-context", get(account_context))
        .route("/api/v1/adapter/user/wallets", get(wallets))
        .with_state((seen_polls, poll_answer.clone()));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn login_flow_polls_until_the_token_arrives() {
    let _env = env_guard::ENV_LOCK.lock().unwrap();
    let (_dir, home) = temp_home("qwen-login-1");
    std::env::set_var("LOCAL_ROUTER_HOME", &home);
    // The endpoint env var points the client at the mock.
    std::env::remove_var("LOCAL_ROUTER_QWEN_ENDPOINT");

    let polls = Arc::new(AtomicUsize::new(0));
    let selects = Arc::new(AtomicUsize::new(0));
    // The first two polls say "still waiting" (no token), the third hands
    // the device token over.
    let answer = Arc::new(Mutex::new(
        json!({"code": 11217, "message": "login ing..."}),
    ));
    let addr = mock_qwen(polls.clone(), selects.clone(), answer.clone()).await;

    std::env::set_var("LOCAL_ROUTER_QWEN_ENDPOINT", format!("http://{addr}"));
    let guard = EndpointGuard;

    let session = local_router::qwen::login::start_login().await.unwrap();
    // The link is built locally: the browser opens it, we never fetch it.
    assert!(
        session.auth_url.starts_with("http://"),
        "url was {}",
        session.auth_url
    );
    assert!(session.auth_url.contains("/device/selectAccounts"));
    assert!(session
        .auth_url
        .contains("redirect_uri=qwenwork-cn%3A%2F%2F"));
    assert!(session.auth_url.contains("challenge_method=S256"));
    assert!(!session.state.verifier.is_empty());

    // Poll twice with no answer, then hand the token over.
    let mut got = None;
    for round in 0..3 {
        if round == 2 {
            *answer.lock().unwrap() = json!({
                "token": "dev-tok", "refresh_token": "dev-ref",
                "expires_in": 3600, "user_id": "u-9"
            });
        }
        match local_router::qwen::login::poll_login(&session.state)
            .await
            .unwrap()
        {
            LoginPoll::Waiting => continue,
            LoginPoll::Ready(t) => {
                got = Some(*t);
                break;
            }
        }
    }
    let token = got.expect("the poll should have produced a token");
    assert_eq!(token.token, "dev-tok");
    assert!(polls.load(Ordering::SeqCst) >= 3);

    // The account read rides the fresh token and returns the user.
    let account = local_router::qwen::login::login_account(&session.state, &token)
        .await
        .unwrap();
    assert_eq!(account.pointer("/data/user/name").unwrap(), "Bob");

    // Storing it lands in the home with 0600 and reads back.
    let saved = local_router::qwen::login::store_session(
        &token,
        &account,
        &session.state.machine_id,
        "default",
    )
    .await
    .unwrap();
    assert_eq!(saved.slot, "default");
    assert!(!saved.same_uid_elsewhere);
    assert!(saved.path.starts_with(&home));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&saved.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o666);
    }
    let stored = local_router::qwen::login::read_session_file().unwrap();
    assert_eq!(stored.device_token, "dev-tok");
    assert_eq!(stored.account.email, "bob@example.com");
    assert_eq!(stored.machine_id, session.state.machine_id);

    drop(guard);
    std::env::remove_var("LOCAL_ROUTER_QWEN_ENDPOINT");
}
struct EndpointGuard;
impl Drop for EndpointGuard {
    fn drop(&mut self) {
        std::env::remove_var("LOCAL_ROUTER_QWEN_ENDPOINT");
    }
}

#[tokio::test]
async fn an_expiring_token_refreshes_before_use() {
    let _env = env_guard::ENV_LOCK.lock().unwrap();
    let (_dir, home) = temp_home("qwen-refresh-1");
    std::env::set_var("LOCAL_ROUTER_HOME", &home);

    let polls = Arc::new(AtomicUsize::new(0));
    let selects = Arc::new(AtomicUsize::new(0));
    let answer = Arc::new(Mutex::new(json!({"token": "x"})));
    let addr = mock_qwen(polls, selects, answer).await;
    std::env::set_var("LOCAL_ROUTER_QWEN_ENDPOINT", format!("http://{addr}"));

    let session = store_session(&home, "tok-old", "ref-old", 30);
    assert!(session.needs_refresh());

    let fresh = local_router::qwen::login::session().await.unwrap();
    assert_eq!(fresh.device_token, "tok-new");
    assert_eq!(fresh.refresh_token, "ref-new");
    assert!(!fresh.needs_refresh());
    // The refreshed token is returned and persisted to the same account slot
    // so a restart does not fall back to the expired token.
    let on_disk = local_router::qwen::login::read_session_file().unwrap();
    assert_eq!(on_disk.device_token, "tok-new");

    std::env::remove_var("LOCAL_ROUTER_QWEN_ENDPOINT");
}

#[tokio::test]
async fn a_healthy_token_skips_the_refresh() {
    let _env = env_guard::ENV_LOCK.lock().unwrap();
    let (_dir, home) = temp_home("qwen-refresh-2");
    std::env::set_var("LOCAL_ROUTER_HOME", &home);

    let polls = Arc::new(AtomicUsize::new(0));
    let selects = Arc::new(AtomicUsize::new(0));
    let answer = Arc::new(Mutex::new(json!({"token": "x"})));
    let addr = mock_qwen(polls, selects, answer).await;
    std::env::set_var("LOCAL_ROUTER_QWEN_ENDPOINT", format!("http://{addr}"));

    store_session(&home, "tok-good", "ref-good", 3600);
    let used = local_router::qwen::login::session().await.unwrap();
    assert_eq!(used.device_token, "tok-good");

    std::env::remove_var("LOCAL_ROUTER_QWEN_ENDPOINT");
}

// ---------------------------------------------------------------- models ---

#[tokio::test]
async fn models_read_the_qwork_scene() {
    let _env = env_guard::ENV_LOCK.lock().unwrap();
    let (_dir, home) = temp_home("qwen-models-1");
    std::env::set_var("LOCAL_ROUTER_HOME", &home);
    let session = store_session(&home, "tok", "ref", 3600);

    // A mock of the signed model list.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    std::env::set_var("LOCAL_ROUTER_QWEN_ENDPOINT", format!("http://{addr}"));
    let server = tokio::spawn(async move {
        use axum::http::{Request, StatusCode};
        use axum::response::IntoResponse;
        let app: axum::Router<()> = axum::Router::new().route(
                "/algo/api/v2/model/list",
                axum::routing::get(|req: Request<axum::body::Body>| async move {
                    // The model list is a signed COSY call: the envelope
                    // headers must be there.
                    let scene = req.headers().get("cosy-scene").map(|v| v.to_str().unwrap().to_string());
                    let client_type = req.headers().get("cosy-clienttype").map(|v| v.to_str().unwrap().to_string());
                    let auth = req.headers().get("authorization").map(|v| v.to_str().unwrap().to_string());
                    if scene.as_deref() != Some("qwork")
                        || client_type.as_deref() != Some("6")
                        || !auth.unwrap_or_default().starts_with("Bearer COSY.")
                    {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    axum::Json(json!({
                        "qwork": [
                            {"key": "flash", "display_name": "标准", "enable": true},
                            {"key": "pro", "display_name": "高级", "enable": false},
                            {"key": "auto", "display_name": "Auto", "enable": true},
                            {"key": "qwen3.8-max-preview", "display_name": "Qwen3.8-Max", "enable": true}
                        ]
                    }))
                    .into_response()
                }),
            );
        axum::serve(listener, app).await.unwrap();
    });

    let models = local_router::qwen::client::models(&session).await.unwrap();
    // The whole qwork catalog is kept (listAllModels), minus auto/default;
    // a disabled model stays, so `pro` is in.
    assert_eq!(models, vec!["flash", "pro", "qwen3.8-max-preview"]);
    server.abort();
    std::env::remove_var("LOCAL_ROUTER_QWEN_ENDPOINT");
}

// ------------------------------------------------------------- the gateway --

#[tokio::test]
async fn the_gateway_routes_a_qwenwork_provider_through_the_signed_path() {
    let _env = env_guard::ENV_LOCK.lock().unwrap();
    let (_dir, home) = temp_home("qwen-gw-1");
    std::env::set_var("LOCAL_ROUTER_HOME", &home);
    let _session = store_session(&home, "dev-tok", "dev-ref", 3600);

    // The mock vendor: a signed chat that answers one SSE envelope.
    let seen_bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_headers: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = seen_bodies.clone();
    let hs_thread = seen_headers.clone();
    std::env::set_var("LOCAL_ROUTER_QWEN_ENDPOINT", format!("http://{addr}"));
    let server = tokio::spawn(async move {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use axum::response::{IntoResponse, Response};
        let bodies = bodies.clone();
        let hs2 = hs_thread.clone();
        let app: axum::Router<()> = axum::Router::new().route(
                "/algo/api/v2/service/pro/sse/agent_chat_generation",
                axum::routing::post(move |req: Request<Body>| {
                    let bodies = bodies.clone();
                    let headers_seen = hs2.clone();
                    async move {
                        let auth = req.headers().get("authorization").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
                        let scene = req.headers().get("cosy-scene").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
                        let model_key = req
                            .headers()
                            .get("x-model-key")
                            .map(|v| v.to_str().unwrap().to_string())
                            .unwrap_or_default();
                        headers_seen.lock().unwrap().push(format!("{auth}|{scene}"));
                        let raw = axum::body::to_bytes(req.into_body(), 4 * 1024 * 1024)
                            .await
                            .unwrap_or_default();
                        bodies.lock().unwrap().push(String::from_utf8_lossy(&raw).to_string());
                        if !auth.starts_with("Bearer COSY.") || scene != "qwork" {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        // The wire body decodes to the plaintext chat body.
                        let wire = bodies.lock().unwrap().last().cloned().unwrap_or_default();
                        let plain = local_router::qwen::cosy::decode_body(&wire).unwrap();
                        let body: Value = serde_json::from_str(&plain).unwrap();
                        assert_eq!(body["session_type"], "qoder_work");
                        assert_eq!(body["parameters"]["enable_thinking"], false);
                        // One SSE frame whose inner body is an OpenAI chunk.
                        let chunk = json!({
                            "id": "c-1", "model": model_key,
                            "choices": [{"index": 0, "delta": {"content": "你好"}, "finish_reason": null}]
                        });
                        let usage_chunk = json!({
                            "id": "c-1", "model": model_key,
                            "choices": [], "usage": {"prompt_tokens": 7, "completion_tokens": 3}
                        });
                        let done = json!({});
                        let sse = format!(
                            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                            json!({"statusCodeValue": 200, "body": chunk.to_string()}),
                            json!({"statusCodeValue": 200, "body": usage_chunk.to_string()}),
                        );
                        let _ = done;
                        let resp = Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(sse))
                            .unwrap();
                        // Keep the stream alive long enough for the reader.
                        let _ = std::io::stdout().flush();
                        resp
                    }
                }),
            );
        axum::serve(listener, app).await.unwrap();
    });

    // The gateway with one qwenwork provider.
    let mut config = Config {
        providers: vec![],
        groups: vec![],
        settings: Default::default(),
        caller_keys: vec![],
    };
    config.settings.default_caller_key = "gw-key".into();
    let mut provider = Provider {
        id: "qwenwork".into(),
        name: "QwenWork".into(),
        auth: Some(local_router::config::AuthKind::QwenWork),
        protocol: local_router::protocol::ir::Protocol::QwenWork,
        stream_only: true,
        models: vec!["flash".into()],
        ..Default::default()
    };
    provider.base_url = Some("https://gateway.qwenwork.cn".into());
    config.providers.push(provider);
    let store = Arc::new(Store::at(home.join("config.json")));
    // Write the config through the store so load() reads what we built.
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    let state = AppState::load(store).await.unwrap();
    let app = local_router::gateway::router(local_router::gateway::Gateway::new(state));

    let listener2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = listener2.local_addr().unwrap();
    let server2 = tokio::spawn(async move {
        axum::serve(listener2, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{gw_addr}/v1/chat/completions"))
        .header("authorization", "Bearer gw-key")
        .json(&json!({"model": "qwenwork/flash", "stream": false, "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or_default();
    assert!(status.is_success(), "gateway answered {status}: {body}");
    // The stream-only provider was folded into a whole reply.
    let text = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(text, "你好");
    assert_eq!(body["usage"]["prompt_tokens"], 7);

    // The vendor saw the COSY envelope and the encoded body.
    let hs = seen_headers.lock().unwrap();
    assert!(hs
        .iter()
        .any(|h| h.starts_with("Bearer COSY.") && h.ends_with("|qwork")));
    let bs = seen_bodies.lock().unwrap();
    assert!(!bs.is_empty());
    assert!(
        bs.iter().all(|b| !b.contains("\"messages\"")),
        "the wire body must be encoded"
    );

    server2.abort();
    server.abort();
    std::env::remove_var("LOCAL_ROUTER_QWEN_ENDPOINT");
}

/// Silence an unused-import warning for the helper above.
#[allow(dead_code)]
fn keep_writes_alive() {
    let _ = std::io::stdout().flush();
}

#[allow(dead_code)]
fn unused_command() {
    let _ = Command::new("true").output();
}

mod tempdir {
    /// A drop-in temp dir that removes itself.
    pub struct TempDir(std::path::PathBuf);
    impl TempDir {
        pub fn new() -> std::io::Result<Self> {
            let base = std::env::temp_dir();
            let name = format!(
                "local-router-qwen-test-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            );
            let path = base.join(name);
            std::fs::create_dir_all(&path)?;
            Ok(TempDir(path))
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A base64 helper for the envelope assertions.
fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .unwrap_or_default()
}
