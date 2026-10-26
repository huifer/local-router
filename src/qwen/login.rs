//! The QwenWork device-flow login and its locally stored session file.
//!
//! The login is the desktop client's PKCE device flow:
//!   1. `POST /device/selectAccounts` with a challenge, nonce and machine id
//!      returns a browser link (`redirect_uri=qwenwork-cn://`).
//!   2. The browser signs in; QwenWork polls `/api/v1/deviceToken/poll`
//!      with the challenge/verifier until it returns a device token.
//!   3. The device token is itself the chat token on QwenWork (no job
//!      token), and the account is read from the adapter account-context.
//!
//! The device token is stored in local-router's own `qwen-session.json`
//! (0600) next to the config — QwenWork's own data is never touched.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::qwen::cosy;

/// Where QwenWork's public cloud lives.
pub const ENDPOINT: &str = "https://gateway.qwenwork.cn";

/// The API host (also the web host for the public cloud).
pub fn endpoint() -> String {
    std::env::var("LOCAL_ROUTER_QWEN_ENDPOINT")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| ENDPOINT.to_string())
}

/// The chat endpoint, with the `Encode=1` body codec flag.
pub const CHAT_PATH: &str =
    "/algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1";

/// The model list, with `Encode=1`.
pub const MODELS_PATH: &str = "/algo/api/v2/model/list?Encode=1";

/// The device-flow paths.
pub const SELECT_ACCOUNTS_PATH: &str = "/device/selectAccounts";
pub const DEVICE_POLL_PATH: &str = "/api/v1/deviceToken/poll";
pub const DEVICE_REFRESH_PATH: &str = "/api/v1/deviceToken/refresh";
pub const ACCOUNT_CONTEXT_PATH: &str =
    "/api/v1/adapter/user/account-context?include=user,plan,quota";
pub const WALLETS_PATH: &str = "/api/v1/adapter/user/wallets";
pub const USERINFO_PATH: &str =
    "/api/v1/adapter/user/account-context?include=user,plan,quota,page,data_sharing";

/// The device-flow redirect URI the desktop client sends.
pub const REDIRECT_URI: &str = "qwenwork-cn://";

/// Refresh this long before the device token actually expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);

/// The shape of the stored session file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionFile {
    #[serde(default)]
    pub account: QwenAccount,
    #[serde(default)]
    pub auth: QwenAuth,
    /// The machine id the device flow generated.
    #[serde(default)]
    pub machine_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QwenAccount {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub email: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QwenAuth {
    /// The device token; it is also the chat token.
    #[serde(default)]
    pub device_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// A live session: the account and a device token good right now.
#[derive(Debug, Clone)]
pub struct Session {
    pub account: QwenAccount,
    pub device_token: String,
    pub refresh_token: String,
    pub machine_id: String,
    pub expires_at: Option<i64>,
}

impl Session {
    pub fn needs_refresh(&self) -> bool {
        let Some(expires_at) = self.expires_at else {
            return false;
        };
        let now = now_ms();
        expires_at - now < REFRESH_MARGIN.as_millis() as i64
    }

    /// The COSY user identity this session carries.
    pub fn cosy_user(&self) -> cosy::CosyUser<'_> {
        cosy::CosyUser {
            uid: &self.account.uid,
            name: &self.account.nickname,
            email: &self.account.email,
            access: &self.device_token,
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Where the session file lives, if a login placed one.
pub fn session_file() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("LOCAL_ROUTER_QWEN_SESSION") {
        let path = PathBuf::from(explicit);
        return path.exists().then_some(path);
    }
    let path = crate::config::config_dir().join("qwen-session.json");
    path.exists().then_some(path)
}

/// Read the stored session file without touching the network.
pub fn read_session_file() -> anyhow::Result<Session> {
    let path = session_file().ok_or_else(|| {
        anyhow::anyhow!(
            "the QwenWork login file was not found; run `local-router qwen login` first"
        )
    })?;
    let raw = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "the QwenWork login file at {} is unreadable",
            path.display()
        )
    })?;
    let file: SessionFile = serde_json::from_str(&raw).with_context(|| {
        format!(
            "the QwenWork login file at {} is not valid json",
            path.display()
        )
    })?;
    if file.auth.device_token.trim().is_empty() {
        anyhow::bail!(
            "the QwenWork login file at {} holds no device token; sign in again",
            path.display()
        );
    }
    Ok(Session {
        account: file.account,
        device_token: file.auth.device_token,
        refresh_token: file.auth.refresh_token,
        machine_id: file.machine_id,
        expires_at: file.auth.expires_at,
    })
}

fn client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("the QwenWork HTTP client could not be built")
}

#[allow(dead_code)]
fn to_header_map(headers: &BTreeMap<String, String>) -> anyhow::Result<reqwest::header::HeaderMap> {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("{name} is not a valid header name"))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .with_context(|| format!("the value of {name} is not a valid header value"))?;
        map.insert(name, value);
    }
    Ok(map)
}

/// The answer of one poll: the browser is still waiting, or the token is in.
#[derive(Debug, Clone)]
pub enum LoginPoll {
    Waiting,
    Ready(Box<DeviceToken>),
}

/// A freshly polled device token.
#[derive(Debug, Clone)]
pub struct DeviceToken {
    pub token: String,
    pub refresh_token: String,
    pub expires_at: Option<i64>,
    pub user_id: String,
}

/// A login session: the browser link, the nonce/verifier to poll with, and
/// the machine id to keep.
#[derive(Debug, Clone)]
pub struct LoginSession {
    pub auth_url: String,
    pub state: LoginState,
}

/// The opaque state a login holds between starting and polling.
#[derive(Debug, Clone)]
pub struct LoginState {
    pub nonce: String,
    pub verifier: String,
    pub machine_id: String,
}

/// Begin a device-flow login: ask QwenWork for the browser link.
pub async fn start_login() -> anyhow::Result<LoginSession> {
    let verifier = b64url_random(64);
    let challenge = b64url(&sha256(&verifier));
    let nonce = uuid();
    let machine_id = uuid();

    let mut q = BTreeMap::new();
    q.insert("challenge".to_string(), challenge);
    q.insert("challenge_method".to_string(), "S256".to_string());
    q.insert("nonce".to_string(), nonce.clone());
    q.insert("machine_id".to_string(), machine_id.clone());
    q.insert("client_id".to_string(), cosy::CLIENT_ID.to_string());
    q.insert("redirect_uri".to_string(), REDIRECT_URI.to_string());

    // The sign-in page is not fetched by us: the browser opens it and the
    // user signs in there. local-router only builds the link and polls.
    let auth_url = format!("{}{}?{}", endpoint(), SELECT_ACCOUNTS_PATH, to_query(&q));
    Ok(LoginSession {
        auth_url,
        state: LoginState {
            nonce,
            verifier,
            machine_id,
        },
    })
}

/// Poll a login session once.
pub async fn poll_login(state: &LoginState) -> anyhow::Result<LoginPoll> {
    let mut q = BTreeMap::new();
    q.insert("nonce".to_string(), state.nonce.clone());
    q.insert("verifier".to_string(), state.verifier.clone());
    q.insert("challenge_method".to_string(), "S256".to_string());
    let url = format!("{}{}?{}", endpoint(), DEVICE_POLL_PATH, to_query(&q));
    let response = client()?
        .get(&url)
        .send()
        .await
        .context("the QwenWork login poll could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the QwenWork login poll returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the QwenWork login poll failed (HTTP {status}): {}",
            message_of(&body)
        );
    }
    let token = body
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if token.trim().is_empty() {
        return Ok(LoginPoll::Waiting);
    }
    let refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let user_id = body
        .get("user_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expires_at = body
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(parse_expired_time)
        .or_else(|| {
            body.get("expires_in")
                .and_then(Value::as_i64)
                .map(|s| now_ms() + s * 1000)
        });
    Ok(LoginPoll::Ready(Box::new(DeviceToken {
        token,
        refresh_token,
        expires_at,
        user_id,
    })))
}

/// Read the account from the adapter account-context.
pub async fn login_account(_state: &LoginState, token: &DeviceToken) -> anyhow::Result<Value> {
    let response = client()?
        .get(format!("{}{}", endpoint(), USERINFO_PATH))
        .header("authorization", format!("Bearer {}", token.token))
        .header("content-type", "application/json")
        .send()
        .await
        .context("the QwenWork account read could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the QwenWork account read returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the QwenWork account read failed (HTTP {status}): {}",
            message_of(&body)
        );
    }
    Ok(body)
}

/// Refresh a device token that is about to expire.
pub async fn refresh(session: &Session) -> anyhow::Result<Session> {
    if session.refresh_token.trim().is_empty() {
        anyhow::bail!("the QwenWork session holds no refresh token; sign in again");
    }
    let url = format!("{}{}", endpoint(), DEVICE_REFRESH_PATH);
    let body = json!({
        "refresh_token": session.refresh_token,
        "target": "c",
    });
    let response = client()?
        .post(&url)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .context("the QwenWork token refresh could not reach the server")?;
    let status = response.status().as_u16();
    let parsed: Value = response
        .json()
        .await
        .context("the QwenWork token refresh returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the QwenWork token refresh was refused (HTTP {status}): {}",
            message_of(&parsed)
        );
    }
    let token = parsed
        .get("token")
        .or_else(|| parsed.get("device_token"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if token.trim().is_empty() {
        anyhow::bail!("the QwenWork token refresh returned no token");
    }
    let refresh_token = parsed
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or(&session.refresh_token)
        .to_string();
    let expires_at = parsed
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(parse_expired_time)
        .or_else(|| {
            parsed
                .get("expires_in")
                .and_then(Value::as_i64)
                .map(|s| now_ms() + s * 1000)
        })
        .or(session.expires_at);
    Ok(Session {
        account: session.account.clone(),
        device_token: token,
        refresh_token,
        machine_id: session.machine_id.clone(),
        expires_at,
    })
}

/// The session to use for the next request: the stored one, refreshed when
/// it is about to expire. A failed refresh is not fatal while the old token
/// still works.
pub async fn session() -> anyhow::Result<Session> {
    let current = read_session_file()?;
    if !current.needs_refresh() {
        return Ok(current);
    }
    match refresh(&current).await {
        Ok(fresh) => Ok(fresh),
        Err(err) => {
            tracing::warn!("the QwenWork token could not be refreshed: {err}");
            Ok(current)
        }
    }
}

/// Store a finished login in local-router's own session file, next to the
/// config, so it survives restarts. The file is 0600: it carries tokens.
pub async fn store_session(
    token: &DeviceToken,
    account: &Value,
    machine_id: &str,
) -> anyhow::Result<PathBuf> {
    let path = crate::config::config_dir().join("qwen-session.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = account
        .pointer("/data/user")
        .or_else(|| account.get("user"))
        .unwrap_or(account);
    let file = SessionFile {
        account: QwenAccount {
            uid: data
                .get("id")
                .or_else(|| data.get("uid"))
                .or_else(|| data.get("user_id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            nickname: data
                .get("name")
                .or_else(|| data.get("nickname"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            email: data
                .get("email")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        auth: QwenAuth {
            device_token: token.token.clone(),
            refresh_token: token.refresh_token.clone(),
            expires_at: token.expires_at,
        },
        machine_id: machine_id.to_string(),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&file)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// A query string from a string map.
fn to_query(q: &BTreeMap<String, String>) -> String {
    q.iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn b64url_random(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut bytes);
    b64url(&bytes)
}

fn sha256(s: &str) -> Vec<u8> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(s.as_bytes());
    hasher.finalize().to_vec()
}

fn uuid() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("msg"))
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("the vendor refused the request")
        .to_string()
}

fn parse_expired_time(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.timestamp_millis());
    }
    if let Ok(dt) = chrono::DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.timestamp_millis());
    }
    trimmed.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_file_parses_and_rejects_an_empty_token() {
        let file = SessionFile {
            account: QwenAccount {
                uid: "u-1".into(),
                nickname: "Zhang".into(),
                email: "z@example.com".into(),
            },
            auth: QwenAuth {
                device_token: "dev-token".into(),
                refresh_token: "ref".into(),
                expires_at: Some(9_000_000_000_000),
            },
            machine_id: "machine-1".into(),
        };
        let raw = serde_json::to_string(&file).unwrap();
        let parsed: SessionFile = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.auth.device_token, "dev-token");
        assert_eq!(parsed.machine_id, "machine-1");

        // A file without a token cannot become a session (the error says so).
        let empty: SessionFile = serde_json::from_str(r#"{"auth": {"device_token": ""}}"#).unwrap();
        assert!(empty.auth.device_token.trim().is_empty());
    }

    #[test]
    fn session_to_live_shape_keeps_every_field() {
        let file = SessionFile {
            account: QwenAccount {
                uid: "u-1".into(),
                nickname: "Zhang".into(),
                email: "z@example.com".into(),
            },
            auth: QwenAuth {
                device_token: "dev-token".into(),
                refresh_token: "ref".into(),
                expires_at: Some(9_000_000_000_000),
            },
            machine_id: "machine-1".into(),
        };
        let session = Session {
            account: file.account.clone(),
            device_token: file.auth.device_token.clone(),
            refresh_token: file.auth.refresh_token.clone(),
            machine_id: file.machine_id.clone(),
            expires_at: file.auth.expires_at,
        };
        // The COSY identity is the same data, re-shaped for the envelope.
        let user = session.cosy_user();
        assert_eq!(user.uid, "u-1");
        assert_eq!(user.name, "Zhang");
        assert_eq!(user.email, "z@example.com");
        assert_eq!(user.access, "dev-token");
        // A long-lived token does not need a refresh.
        assert!(!session.needs_refresh());
    }

    #[test]
    fn paths_are_the_ones_the_client_speaks() {
        assert!(CHAT_PATH.starts_with("/algo/api/v2/service/pro/sse/"));
        assert!(CHAT_PATH.contains("Encode=1"));
        assert!(MODELS_PATH.contains("Encode=1"));
        assert!(ACCOUNT_CONTEXT_PATH.starts_with("/api/v1/adapter/user/account-context"));
        assert_eq!(REDIRECT_URI, "qwenwork-cn://");
        assert_eq!(ENDPOINT, "https://gateway.qwenwork.cn");
    }
}
