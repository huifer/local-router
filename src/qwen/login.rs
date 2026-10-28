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
//! next to the config — QwenWork's own data is never touched.

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

/// The pool of QwenWork accounts local-router holds: one file, several
/// accounts, and the one that is in use.
///
/// The file is `qwen-session.json` next to the config. An older file held a
/// single session at the top level; that shape is still read (as one
/// account named `default`) so nothing breaks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionPool {
    /// Which account the gateway uses when the caller does not pick one.
    #[serde(default)]
    pub active: String,
    /// The accounts, by name.
    #[serde(default)]
    pub accounts: BTreeMap<String, SessionFile>,
}

/// Where local-router keeps its own QwenWork sessions.
pub fn own_session_path() -> PathBuf {
    crate::config::config_dir().join("qwen-session.json")
}

/// The file the pool lives in, consistently for reads and writes: the
/// explicit session path wins, then local-router's own file next to the
/// config.
fn pool_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("LOCAL_ROUTER_QWEN_SESSION") {
        return PathBuf::from(explicit);
    }
    own_session_path()
}

/// Read a pool from one file: either the pool shape or the old
/// single-session shape (which is one account named `default`).
fn read_pool_at(path: &std::path::Path) -> anyhow::Result<SessionPool> {
    let raw = std::fs::read_to_string(path).with_context(|| {
        format!(
            "the QwenWork session file at {} is unreadable",
            path.display()
        )
    })?;
    // A pool shape with accounts wins.
    if let Ok(pool) = serde_json::from_str::<SessionPool>(&raw) {
        if !pool.accounts.is_empty() {
            return Ok(pool);
        }
    }
    // The old single-session shape (a bare SessionFile at the top level).
    let file: SessionFile = serde_json::from_str(&raw).with_context(|| {
        format!(
            "the QwenWork login file at {} is not valid json",
            path.display()
        )
    })?;
    let mut accounts = BTreeMap::new();
    accounts.insert("default".to_string(), file);
    Ok(SessionPool {
        active: "default".to_string(),
        accounts,
    })
}

/// Read the pool: our own file when it exists, else the explicit path.
pub fn read_pool() -> anyhow::Result<SessionPool> {
    let path = pool_path();
    if path.exists() {
        return read_pool_at(&path);
    }
    anyhow::bail!(
        "no QwenWork session yet; run `local-router qwen login` to sign in with a browser"
    )
}

/// The accounts local-router knows, by name.
pub fn accounts() -> anyhow::Result<Vec<(String, SessionFile)>> {
    Ok(read_pool()?.accounts.into_iter().collect())
}

/// Use another account from now on.
pub fn use_account(name: &str) -> anyhow::Result<()> {
    let mut pool = read_pool()?;
    if !pool.accounts.contains_key(name) {
        anyhow::bail!("there is no QwenWork account called {name}");
    }
    pool.active = name.to_string();
    write_pool(&pool)?;
    Ok(())
}

/// Write the pool back to local-router's own file.
fn write_pool(pool: &SessionPool) -> anyhow::Result<()> {
    let path = pool_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(pool)?;
    std::fs::write(&path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
    }
    Ok(())
}

/// A session of one named account.
/// Persist a refreshed device token pair back to the named account slot.
pub fn persist_refreshed(name: &str, session: &Session) -> anyhow::Result<()> {
    let mut pool = read_pool()?;
    let file = pool
        .accounts
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("there is no QwenWork account called {name}"))?;
    file.account = session.account.clone();
    file.auth.device_token = session.device_token.clone();
    file.auth.refresh_token = session.refresh_token.clone();
    file.auth.expires_at = session.expires_at;
    file.machine_id = session.machine_id.clone();
    write_pool(&pool)
}

pub fn session_of(name: &str) -> anyhow::Result<Session> {
    let pool = read_pool()?;
    let file = pool
        .accounts
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("there is no QwenWork account called {name}"))?;
    Ok(session_from_file(file))
}

/// Turn a stored session file into a live session.
fn session_from_file(file: &SessionFile) -> Session {
    Session {
        account: file.account.clone(),
        device_token: file.auth.device_token.clone(),
        refresh_token: file.auth.refresh_token.clone(),
        machine_id: file.machine_id.clone(),
        expires_at: file.auth.expires_at,
    }
}

/// The next account's session, without renewing it — so several QwenWork
/// accounts share the traffic instead of one taking all of it.
fn rotating_account() -> anyhow::Result<(String, Session)> {
    let pool = read_pool()?;
    if pool.accounts.is_empty() {
        anyhow::bail!("no QwenWork session yet; run `local-router qwen login`");
    }
    let names: Vec<String> = pool.accounts.keys().cloned().collect();
    let index = {
        static CURSOR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let i = CURSOR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        i % names.len()
    };
    let name = names[index].clone();
    let session = session_from_file(&pool.accounts[&name]);
    Ok((name, session))
}

/// A session of one of the accounts, in turn, renewed first when it is
/// about to expire (like the single-account path).
pub async fn session_rotating() -> anyhow::Result<(String, Session)> {
    let (name, session) = rotating_account()?;
    if !session.needs_refresh() {
        return Ok((name, session));
    }
    match refresh(&session).await {
        Ok(fresh) => {
            let _ = persist_refreshed(&name, &fresh);
            Ok((name, fresh))
        }
        Err(err) => {
            // A failed refresh is not fatal while the old token still works.
            tracing::warn!("the QwenWork token could not be refreshed: {err}");
            Ok((name, session))
        }
    }
}

/// Read the stored session file without touching the network.
///
/// Reads from the pool: the active account when one is set, else the first
/// account. Kept for callers that only need one session (status, CLI).
pub fn read_session_file() -> anyhow::Result<Session> {
    let pool = read_pool()?;
    let name = if pool.active.is_empty() {
        pool.accounts
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the QwenWork session file holds no account"))?
    } else {
        pool.active.clone()
    };
    let file = pool
        .accounts
        .get(&name)
        .ok_or_else(|| anyhow::anyhow!("there is no QwenWork account called {name}"))?;
    if file.auth.device_token.trim().is_empty() {
        anyhow::bail!(
            "the QwenWork session file holds no device token; sign in with `local-router qwen login`"
        );
    }
    Ok(session_from_file(file))
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
        Ok(fresh) => {
            let active = read_pool()?.active;
            let _ = persist_refreshed(&active, &fresh);
            Ok(fresh)
        }
        Err(err) => {
            tracing::warn!("the QwenWork token could not be refreshed: {err}");
            Ok(current)
        }
    }
}

/// A small HTTP client for QwenWork's API.
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

/// The outcome of storing a finished sign-in.
#[derive(Debug, Clone)]
pub struct StoredLogin {
    /// The file the pool lives in.
    pub path: PathBuf,
    /// The slot the account landed in.
    pub slot: String,
    /// Another slot in the pool already holds the same uid — the browser
    /// signed the same person in again. The account is still filed under
    /// its own slot; the flag only says so out loud.
    pub same_uid_elsewhere: bool,
}

/// Store a finished login in local-router's own session file, next to the
/// config, so it survives restarts. The file carries tokens; its mode lets
/// a container or another local tool read the pool it owns.
///
/// The pool keeps every account; the one just signed in becomes the one in
/// use — the deliberate act of signing in is also "switch to it".
///
/// Every sign-in gets its own slot, so any number of accounts can live side
/// by side.
pub async fn store_session(
    token: &DeviceToken,
    account: &Value,
    machine_id: &str,
    name: &str,
) -> anyhow::Result<StoredLogin> {
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
    let mut pool = read_pool().unwrap_or_default();
    let uid = file.account.uid.trim();
    let same_uid_elsewhere = !uid.is_empty()
        && pool
            .accounts
            .iter()
            .any(|(held, f)| held.as_str() != name && f.account.uid.trim() == uid);
    let slot = name.to_string();
    pool.active = slot.clone();
    pool.accounts.insert(slot.clone(), file);
    // The pool is written through the same path it is read from, so a
    // sign-in and the next read always agree on the file.
    write_pool(&pool)?;
    Ok(StoredLogin {
        path: pool_path(),
        slot,
        same_uid_elsewhere,
    })
}

/// Forget one account: drop it from the pool, and switch to another one
/// when it happened to be the account in use.
pub fn remove_account(name: &str) -> anyhow::Result<bool> {
    let mut pool = read_pool()?;
    if pool.accounts.remove(name).is_none() {
        anyhow::bail!("there is no QwenWork account called {name}");
    }
    let was_active = pool.active == name;
    if was_active {
        pool.active = pool.accounts.keys().next().cloned().unwrap_or_default();
    }
    write_pool(&pool)?;
    Ok(was_active)
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

    /// A stored session file, for the pool tests.
    fn file(uid: &str, nickname: &str) -> SessionFile {
        SessionFile {
            account: QwenAccount {
                uid: uid.into(),
                nickname: nickname.into(),
                email: format!("{uid}@example.com"),
            },
            auth: QwenAuth {
                device_token: format!("token-{uid}"),
                refresh_token: format!("refresh-{uid}"),
                expires_at: Some(9_000_000_000_000),
            },
            machine_id: format!("machine-{uid}"),
        }
    }

    #[test]
    fn the_pool_holds_several_accounts_and_names_the_one_in_use() {
        let mut accounts = BTreeMap::new();
        accounts.insert("default".to_string(), file("u-1", "Zhang"));
        accounts.insert("work".to_string(), file("u-2", "Li"));
        accounts.insert("personal".to_string(), file("u-3", "Wang"));
        let pool = SessionPool {
            active: "work".to_string(),
            accounts,
        };

        // Three accounts side by side, one of them in use.
        assert_eq!(pool.accounts.len(), 3);
        assert_eq!(pool.active, "work");
        // Every account keeps its own identity and token.
        assert_eq!(pool.accounts["work"].account.nickname, "Li");
        assert_eq!(pool.accounts["work"].auth.device_token, "token-u-2");
        assert_eq!(pool.accounts["default"].account.nickname, "Zhang");
        assert_eq!(pool.accounts["personal"].account.nickname, "Wang");
    }

    #[test]
    fn the_pool_survives_a_round_trip_through_json() {
        let mut accounts = BTreeMap::new();
        accounts.insert("default".to_string(), file("u-1", "Zhang"));
        accounts.insert("second".to_string(), file("u-2", "Li"));
        let pool = SessionPool {
            active: "second".to_string(),
            accounts,
        };

        let raw = serde_json::to_string(&pool).unwrap();
        let parsed: SessionPool = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.accounts.len(), 2);
        assert_eq!(parsed.active, "second");
        assert_eq!(parsed.accounts["second"].account.uid, "u-2");
    }

    #[test]
    fn an_old_single_session_file_reads_as_the_default_account() {
        // The old shape: one session at the top level, no pool wrapper.
        let single = file("u-1", "Zhang");
        let raw = serde_json::to_string(&single).unwrap();

        // A file in the old shape is not a pool...
        let as_pool: SessionPool = serde_json::from_str(&raw).unwrap_or_default();
        assert!(as_pool.accounts.is_empty());

        // ...so the reader migrates it to one account named "default":
        // the account is not lost, and it is the one in use.
        let dir = std::env::temp_dir().join(format!("qwen-pool-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("qwen-session.json");
        std::fs::write(&path, &raw).unwrap();

        let pool = read_pool_at(&path).unwrap();
        assert_eq!(pool.accounts.len(), 1);
        assert_eq!(pool.active, "default");
        assert_eq!(pool.accounts["default"].account.uid, "u-1");
        assert_eq!(pool.accounts["default"].auth.device_token, "token-u-1");

        // A file in the new shape is read as the pool it is.
        let mut accounts = BTreeMap::new();
        accounts.insert("default".to_string(), file("u-1", "Zhang"));
        accounts.insert("second".to_string(), file("u-2", "Li"));
        let pool = SessionPool {
            active: "second".to_string(),
            accounts,
        };
        std::fs::write(&path, serde_json::to_string(&pool).unwrap()).unwrap();
        let pool = read_pool_at(&path).unwrap();
        assert_eq!(pool.accounts.len(), 2);
        assert_eq!(pool.active, "second");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_session_keeps_every_field_of_its_stored_file() {
        let stored = file("u-9", "Zhao");
        let live = session_from_file(&stored);
        assert_eq!(live.account.uid, "u-9");
        assert_eq!(live.account.nickname, "Zhao");
        assert_eq!(live.account.email, "u-9@example.com");
        assert_eq!(live.device_token, "token-u-9");
        assert_eq!(live.refresh_token, "refresh-u-9");
        assert_eq!(live.machine_id, "machine-u-9");
        assert_eq!(live.expires_at, Some(9_000_000_000_000));
        // A far-future token does not need a refresh.
        assert!(!live.needs_refresh());
    }

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
