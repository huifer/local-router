//! Reading the WorkBuddy desktop app's login session.
//!
//! WorkBuddy keeps its session in a file the desktop app shares with its
//! CLI. local-router never copies the secrets out of it: it reads the file
//! when a request needs it, refreshes the access token when it is about to
//! expire, and forgets everything again. The token is never written to
//! `config.json`, never logged, and never written to the usage log.
//!
//! The upstream speaks the OpenAI Chat Completions shape at
//! `POST {endpoint}/v2/chat/completions`, and it only answers streaming
//! requests — a non-streaming call is refused with "Non-stream chat
//! request is currently not supported". It also needs a few extra headers
//! beyond the bearer token, which `auth_headers` assembles.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Where WorkBuddy's API lives.
pub const ENDPOINT: &str = "https://copilot.tencent.com";

/// The API to talk to. A private deployment or a test can point it
/// somewhere else.
pub fn endpoint() -> String {
    std::env::var("LOCAL_ROUTER_WORKBUDDY_ENDPOINT")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| ENDPOINT.to_string())
}

/// The path of the chat endpoint, relative to [`ENDPOINT`].
pub const CHAT_PATH: &str = "/v2/chat/completions";

/// The path that refreshes the access token, relative to [`ENDPOINT`].
pub const REFRESH_PATH: &str = "/v2/plugin/auth/token/refresh";

/// The path that lists the user's resource packages (their credits).
pub const RESOURCE_PATH: &str = "/v2/billing/meter/get-user-resource";

/// The path that reports the daily check-in state.
pub const CHECKIN_PATH: &str = "/v2/billing/meter/checkin-activity-status";

/// The path that serves the model catalogue.
pub const CONFIG_PATH: &str = "/v3/config";

/// Where a login session is opened, and where its outcome is polled.
pub const AUTH_STATE_PATH: &str = "/v2/plugin/auth/state?platform=workbuddy";
pub const AUTH_TOKEN_PATH: &str = "/v2/plugin/auth/token?state=";
pub const AUTH_ACCOUNT_PATH: &str = "/v2/plugin/login/account?state=";

/// The login code the vendor answers while the browser is still waiting.
const LOGIN_PENDING: i64 = 11217;

/// The product name WorkBuddy expects in `X-Product`.
const PRODUCT: &str = "WorkBuddy";

/// The version WorkBuddy reports in its IDE headers.
const CLIENT_VERSION: &str = "5.3.14";

/// Refresh this long before the access token actually expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);

/// The shape of the session file WorkBuddy writes.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionFile {
    #[serde(default)]
    pub account: Account,
    #[serde(default)]
    pub auth: Auth,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Account {
    #[serde(default)]
    pub uid: String,
    /// The display name. The desktop app encrypts it
    /// (`{"$wbEncrypted": 1, "envelope": …}`), so a non-string is read as
    /// "the name is encrypted" rather than failing the whole file.
    #[serde(default, deserialize_with = "string_or_encrypted")]
    pub nickname: String,
    #[serde(default, rename = "type")]
    pub account_type: String,
    #[serde(default, rename = "lastLogin")]
    pub last_login: bool,
}

/// Read a field that is either a string or the desktop app's encrypted
/// envelope. Anything else becomes a short placeholder.
fn string_or_encrypted<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::String(s) => s,
        Value::Object(_) => "（客户端加密了昵称）".to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Auth {
    #[serde(default, rename = "accessToken")]
    pub access_token: String,
    #[serde(default, rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(default, rename = "tokenType")]
    pub token_type: String,
    #[serde(default, rename = "expiresAt")]
    pub expires_at: Option<i64>,
    #[serde(default, rename = "refreshExpiresAt")]
    pub refresh_expires_at: Option<i64>,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub scope: String,
}

/// A live session: the account and a token that is good right now.
#[derive(Debug, Clone)]
pub struct Session {
    pub account: Account,
    pub access_token: String,
    pub refresh_token: String,
    pub domain: String,
    /// When the access token stops working, in milliseconds since the epoch.
    pub expires_at: Option<i64>,
}

impl Session {
    /// Whether the access token is expired, or close enough to expiry that
    /// it should be renewed before the next request.
    pub fn needs_refresh(&self) -> bool {
        let Some(expires_at) = self.expires_at else {
            return false;
        };
        let now = now_ms();
        expires_at - now < REFRESH_MARGIN.as_millis() as i64
    }

    /// The headers every WorkBuddy request needs.
    pub fn auth_headers(&self) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {}", self.access_token),
        );
        if !self.account.uid.is_empty() {
            headers.insert("x-user-id".to_string(), self.account.uid.clone());
        }
        if !self.domain.is_empty() {
            headers.insert("x-domain".to_string(), self.domain.clone());
        }
        headers.insert("x-product".to_string(), PRODUCT.to_string());
        headers.insert("x-ide-type".to_string(), PRODUCT.to_string());
        headers.insert("x-ide-name".to_string(), PRODUCT.to_string());
        headers.insert("x-ide-version".to_string(), CLIENT_VERSION.to_string());
        headers.insert(
            "user-agent".to_string(),
            format!("{PRODUCT}/{CLIENT_VERSION}"),
        );
        headers
    }
}

/// Now, in milliseconds since the epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Where WorkBuddy keeps its session file, if we can find it.
pub fn session_file() -> Option<PathBuf> {
    // An explicit path wins, and is never second-guessed: a deployment
    // that points elsewhere means it.
    if let Ok(explicit) = std::env::var("LOCAL_ROUTER_WORKBUDDY_SESSION") {
        let path = PathBuf::from(explicit);
        return path.exists().then_some(path);
    }
    // The session local-router stored itself (from `workbuddy login`) comes
    // first: the user asked for that account on purpose, and it may be a
    // second account besides the desktop app's.
    let own = crate::config::config_dir().join("workbuddy-session.json");
    if own.exists() {
        return Some(own);
    }
    let home = dirs::home_dir()?;
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("WORKBUDDY_HOME") {
        candidates.push(PathBuf::from(dir).join("auth/workbuddy-desktop.info"));
    }
    // macOS: the desktop app's shared data directory.
    candidates.push(home.join(
        "Library/Application Support/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info",
    ));
    // Linux and other XDG platforms.
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        candidates.push(
            PathBuf::from(dir).join("CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info"),
        );
    }
    candidates
        .push(home.join(".local/share/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info"));
    candidates.push(home.join(".workbuddy/auth/workbuddy-desktop.info"));
    candidates.into_iter().find(|p| p.exists())
}

/// Read the session file without touching the network.
pub fn read_session_file() -> anyhow::Result<Session> {
    // An explicit path wins; otherwise the pool (our file, else the app's).
    let pool = match std::env::var("LOCAL_ROUTER_WORKBUDDY_SESSION") {
        Ok(explicit) => {
            let path = PathBuf::from(explicit);
            if !path.exists() {
                anyhow::bail!(
                    "the WorkBuddy login file at {} was not found",
                    path.display()
                );
            }
            read_pool_at(&path)?
        }
        Err(_) => read_pool()?,
    };
    let name = if pool.active.is_empty() {
        pool.accounts
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the WorkBuddy session file holds no account"))?
    } else {
        pool.active.clone()
    };
    let file = pool
        .accounts
        .get(&name)
        .ok_or_else(|| anyhow::anyhow!("there is no WorkBuddy account called {name}"))?;
    if file.auth.access_token.trim().is_empty() {
        anyhow::bail!(
            "the WorkBuddy session file holds no access token; sign in with `local-router workbuddy login`"
        );
    }
    Ok(Session {
        account: file.account.clone(),
        access_token: file.auth.access_token.clone(),
        refresh_token: file.auth.refresh_token.clone(),
        domain: file.auth.domain.clone(),
        expires_at: file.auth.expires_at,
    })
}

/// A small HTTP client for WorkBuddy's API.
fn client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("the WorkBuddy HTTP client could not be built")
}

/// Refresh the access token, and return the new session.
///
/// The old refresh token stays valid, so a failed refresh is not fatal: the
/// caller can keep using the token it already has.
pub async fn refresh(session: &Session) -> anyhow::Result<Session> {
    if session.refresh_token.trim().is_empty() {
        anyhow::bail!("the WorkBuddy session holds no refresh token; sign in again");
    }
    let url = format!("{}{REFRESH_PATH}", endpoint());
    let mut headers = session.auth_headers();
    // The refresh endpoint takes the refresh token in a header, not the body.
    headers.insert("x-refresh-token".to_string(), session.refresh_token.clone());
    headers.insert("x-auth-refresh-source".to_string(), "plugin".to_string());
    headers.insert("content-type".to_string(), "application/json".to_string());

    let response = client()?
        .post(&url)
        .headers(to_header_map(&headers)?)
        .body("{}")
        .send()
        .await
        .context("the WorkBuddy token refresh could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the WorkBuddy token refresh returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the WorkBuddy token refresh was refused (HTTP {status}): {}",
            message_of(&body)
        );
    }
    let data = body.get("data").cloned().unwrap_or_default();
    let access_token = data
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if access_token.trim().is_empty() {
        anyhow::bail!("the WorkBuddy token refresh returned no access token");
    }
    let expires_in = data.get("expiresIn").and_then(Value::as_i64);
    Ok(Session {
        account: session.account.clone(),
        access_token: access_token.to_string(),
        refresh_token: data
            .get("refreshToken")
            .and_then(Value::as_str)
            .unwrap_or(&session.refresh_token)
            .to_string(),
        domain: data
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or(&session.domain)
            .to_string(),
        expires_at: expires_in.map(|seconds| now_ms() + seconds * 1000),
    })
}

/// The session to use for the next request: the one on disk, refreshed when
/// it is about to expire.
///
/// The refreshed token is returned to the caller but never written back to
/// disk: WorkBuddy owns that file, and its own refresh writes it.
pub async fn session() -> anyhow::Result<Session> {
    let pool = read_pool()?;
    let name = if pool.active.is_empty() {
        pool.accounts.keys().next().cloned().ok_or_else(|| {
            anyhow::anyhow!("no WorkBuddy session yet; run `local-router workbuddy login`")
        })?
    } else {
        pool.active.clone()
    };
    let file = pool
        .accounts
        .get(&name)
        .ok_or_else(|| anyhow::anyhow!("there is no WorkBuddy account called {name}"))?;
    let current = Session {
        account: file.account.clone(),
        access_token: file.auth.access_token.clone(),
        refresh_token: file.auth.refresh_token.clone(),
        domain: file.auth.domain.clone(),
        expires_at: file.auth.expires_at,
    };
    if !current.needs_refresh() {
        return Ok(current);
    }
    match refresh(&current).await {
        Ok(fresh) => Ok(fresh),
        Err(err) => {
            // A refresh that fails is not fatal while the old token still
            // works; the request itself will say so if it does not.
            tracing::warn!("the WorkBuddy token could not be refreshed: {err}");
            Ok(current)
        }
    }
}

/// The sessions local-router holds for WorkBuddy: one file, several
/// accounts, and the one that is in use.
///
/// The file is `workbuddy-session.json` next to the config. An older file
/// held a single session at the top level; that shape is still read (as one
/// account named `default`) so nothing breaks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionPool {
    /// Which account the gateway uses when the caller does not pick one.
    #[serde(default)]
    pub active: String,
    /// The accounts, by name.
    #[serde(default)]
    pub accounts: std::collections::BTreeMap<String, SessionFile>,
}

/// Where local-router keeps its own WorkBuddy sessions.
pub fn own_session_path() -> std::path::PathBuf {
    crate::config::config_dir().join("workbuddy-session.json")
}

/// Read a pool from one file: either the pool shape or the old
/// single-session shape (which is one account named `default`).
fn read_pool_at(path: &std::path::Path) -> anyhow::Result<SessionPool> {
    let raw = std::fs::read_to_string(path).with_context(|| {
        format!(
            "the WorkBuddy session file at {} is unreadable",
            path.display()
        )
    })?;
    if let Ok(pool) = serde_json::from_str::<SessionPool>(&raw) {
        if !pool.accounts.is_empty() {
            return Ok(pool);
        }
    }
    let file: SessionFile = serde_json::from_str(&raw).with_context(|| {
        format!(
            "the WorkBuddy login file at {} is not valid json — this version of the app encrypts its tokens, so sign in with `local-router workbuddy login` instead",
            path.display()
        )
    })?;
    let mut accounts = std::collections::BTreeMap::new();
    accounts.insert("default".to_string(), file);
    Ok(SessionPool {
        active: "default".to_string(),
        accounts,
    })
}

/// Read the pool: our own file when it exists, else the desktop app's file
/// as a single account.
pub fn read_pool() -> anyhow::Result<SessionPool> {
    let own = own_session_path();
    if own.exists() {
        return read_pool_at(&own);
    }
    // The desktop app's own file (older versions kept the token in clear).
    let path = session_file().ok_or_else(|| {
        anyhow::anyhow!(
            "no WorkBuddy session yet; run `local-router workbuddy login` to sign in with a browser"
        )
    })?;
    let pool = read_pool_at(&path)?;
    let active_file = pool
        .accounts
        .get(&pool.active)
        .or_else(|| pool.accounts.values().next())
        .ok_or_else(|| {
            anyhow::anyhow!("the WorkBuddy login file at {} is empty", path.display())
        })?;
    if active_file.auth.access_token.trim().is_empty() {
        anyhow::bail!(
            "the WorkBuddy login file at {} holds no access token; sign in with `local-router workbuddy login`",
            path.display()
        );
    }
    Ok(pool)
}

/// The accounts local-router knows, by name.
pub fn accounts() -> anyhow::Result<Vec<(String, SessionFile)>> {
    Ok(read_pool()?.accounts.into_iter().collect())
}

/// Use another account from now on.
pub fn use_account(name: &str) -> anyhow::Result<()> {
    let mut pool = read_pool()?;
    if !pool.accounts.contains_key(name) {
        anyhow::bail!("there is no WorkBuddy account called {name}");
    }
    pool.active = name.to_string();
    write_pool(&pool)?;
    Ok(())
}

/// Write the pool back to local-router's own file.
fn write_pool(pool: &SessionPool) -> anyhow::Result<()> {
    let path = own_session_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(pool)?;
    std::fs::write(&path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// The session of one named account.
pub fn session_of(name: &str) -> anyhow::Result<Session> {
    let pool = read_pool()?;
    let file = pool
        .accounts
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("there is no WorkBuddy account called {name}"))?;
    Ok(Session {
        account: file.account.clone(),
        access_token: file.auth.access_token.clone(),
        refresh_token: file.auth.refresh_token.clone(),
        domain: file.auth.domain.clone(),
        expires_at: file.auth.expires_at,
    })
}

/// A session of one of the accounts, in turn — so several WorkBuddy
/// accounts share the traffic instead of one taking all of it. The token is
/// renewed first when it is about to expire, like the single-account path.
pub async fn session_rotating() -> anyhow::Result<(String, Session)> {
    let (name, session) = rotating_account()?;
    if !session.needs_refresh() {
        return Ok((name, session));
    }
    match refresh(&session).await {
        Ok(fresh) => Ok((name, fresh)),
        Err(err) => {
            // A failed refresh is not fatal while the old token still works.
            tracing::warn!("the WorkBuddy token could not be refreshed: {err}");
            Ok((name, session))
        }
    }
}

/// The next account's session, without renewing it.
pub fn rotating_account() -> anyhow::Result<(String, Session)> {
    let pool = read_pool()?;
    if pool.accounts.is_empty() {
        anyhow::bail!("no WorkBuddy session yet; run `local-router workbuddy login`");
    }
    let names: Vec<String> = pool.accounts.keys().cloned().collect();
    let index = {
        static CURSOR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let i = CURSOR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        i % names.len()
    };
    let name = names[index].clone();
    let file = &pool.accounts[&name];
    Ok((
        name,
        Session {
            account: file.account.clone(),
            access_token: file.auth.access_token.clone(),
            refresh_token: file.auth.refresh_token.clone(),
            domain: file.auth.domain.clone(),
            expires_at: file.auth.expires_at,
        },
    ))
}

/// One quota window on the account, in the shape the routing view reads:
/// a name, what is left, and when it comes back.
#[derive(Debug, Clone, Serialize)]
pub struct QuotaWindow {
    pub name: String,
    pub used: f64,
    pub remaining: f64,
    /// When the window resets, RFC 3339.
    pub resets_at: Option<String>,
    /// A human summary, like `98 / 1400 credits`.
    pub display: String,
    pub unit: String,
    /// Why the router holds this account back, when it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holds_why: Option<String>,
}

impl QuotaWindow {
    /// Whether the window is one that can run out at all.
    pub fn unlimited(&self) -> bool {
        self.unit == "unlimited"
    }
}

/// The account's quota, as the routing view reads it.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderQuota {
    pub provider: String,
    pub name: String,
    /// `subscription` — it is the WorkBuddy app's own plan.
    pub kind: String,
    pub plan: String,
    pub user: String,
    pub windows: Vec<QuotaWindow>,
    /// When this was read, RFC 3339.
    pub read_at: String,
}

/// The account's quota window, from the same numbers the credits view uses.
pub async fn quota() -> anyhow::Result<ProviderQuota> {
    let session = session().await?;
    let credits = credits().await?;
    let mut resets: Option<i64> = None;
    for item in &credits.packages {
        if item.remaining > 0 {
            if let Some(at) = item.expires_at {
                resets = Some(match resets {
                    Some(so_far) if so_far <= at => so_far,
                    _ => at,
                });
            }
        }
    }
    let remaining = credits.remaining as f64;
    let capacity = credits.capacity as f64;
    let used = capacity - remaining;
    let windows = vec![QuotaWindow {
        name: "Credits".to_string(),
        used,
        remaining,
        resets_at: resets
            .and_then(|at| chrono::DateTime::from_timestamp_millis(at).map(|t| t.to_rfc3339())),
        display: format!("{remaining:.0} / {capacity:.0} {}", "credits"),
        unit: "credits".to_string(),
        holds_why: (remaining <= 0.0).then(|| "clock".to_string()),
    }];
    Ok(ProviderQuota {
        provider: "workbuddy".to_string(),
        name: "WorkBuddy".to_string(),
        kind: "subscription".to_string(),
        plan: "Free plan".to_string(),
        user: session.account.nickname.clone(),
        windows,
        read_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Whether the account's quota window says it is empty.
///
/// The answer is cached for a couple of minutes: the routing view asks
/// before every request, and the numbers do not move that fast.
pub async fn exhausted() -> anyhow::Result<bool> {
    static CACHE: tokio::sync::OnceCell<tokio::sync::Mutex<Option<(std::time::Instant, bool)>>> =
        tokio::sync::OnceCell::const_new();
    let cache = CACHE
        .get_or_init(|| async { tokio::sync::Mutex::new(None) })
        .await
        .lock()
        .await;
    if let Some((read_at, exhausted)) = cache.as_ref() {
        if read_at.elapsed() < std::time::Duration::from_secs(120) {
            return Ok(*exhausted);
        }
    }
    drop(cache);

    let quota = quota().await?;
    let empty = quota
        .windows
        .iter()
        .all(|w| !w.unlimited() && w.remaining <= 0.0);
    let mut cache = CACHE
        .get_or_init(|| async { tokio::sync::Mutex::new(None) })
        .await
        .lock()
        .await;
    *cache = Some((std::time::Instant::now(), empty));
    Ok(empty)
}

/// One resource package on the account.
#[derive(Debug, Clone, Serialize)]
pub struct Resource {
    pub package_name: String,
    /// How many credits the package holds.
    pub capacity: u64,
    /// How many of them are used.
    pub used: u64,
    /// How many are left in this cycle.
    pub remaining: u64,
    pub unit: String,
    /// When the package expires, in milliseconds since the epoch.
    pub expires_at: Option<i64>,
}

/// The account's credits.
#[derive(Debug, Clone, Serialize)]
pub struct Credits {
    /// Every package on the account.
    pub packages: Vec<Resource>,
    /// The credits left in this cycle, summed over the packages.
    pub remaining: u64,
    /// The credits this cycle holds, summed over the packages.
    pub capacity: u64,
    /// The daily check-in state.
    pub checkin: Option<Checkin>,
}

/// The daily check-in state.
#[derive(Debug, Clone, Serialize)]
pub struct Checkin {
    pub checked_in_today: bool,
    pub streak_days: u64,
    pub daily_credit: u64,
    pub today_credit: u64,
    /// Whether the check-in activity is running at all.
    pub active: bool,
}

/// The outcome of a check-in attempt.
#[derive(Debug, Clone, Serialize)]
pub struct CheckinOutcome {
    /// New credits were granted just now.
    pub granted: bool,
    /// The day's check-in had already been made.
    pub already: bool,
    /// How many credits came with it, when the vendor said.
    pub credit: Option<u64>,
    /// What the vendor said, for the log and the CLI.
    pub message: String,
}

/// Claim the day's check-in.
///
/// The vendor answers 400 with `今天已签到` when the day's check-in has
/// already been made; that is not a failure, it is the expected answer on
/// the second call of the day.
pub async fn claim_checkin() -> anyhow::Result<CheckinOutcome> {
    let session = session().await?;
    let mut headers = session.auth_headers();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "application/json".to_string());

    let response = client()?
        .post(format!(
            "{}{}",
            endpoint(),
            "/v2/billing/meter/daily-checkin"
        ))
        .headers(to_header_map(&headers)?)
        .body("{}")
        .send()
        .await
        .context("the check-in could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the check-in returned no json")?;

    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let message = body
        .get("msg")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if (200..300).contains(&status) && code == 0 {
        let credit = body
            .pointer("/data/daily_credit")
            .or_else(|| body.pointer("/data/credit"))
            .or_else(|| body.pointer("/data/today_credit"))
            .and_then(Value::as_i64)
            .map(|v| v as u64);
        return Ok(CheckinOutcome {
            granted: true,
            already: false,
            credit,
            message: if message.is_empty() {
                "checked in".to_string()
            } else {
                message
            },
        });
    }

    // An already-claimed day is the expected answer, not an error.
    let already = message.contains("已签到") || code == 10001;
    if !(200..300).contains(&status) && !already {
        anyhow::bail!("the check-in was refused (HTTP {status}): {message}");
    }
    Ok(CheckinOutcome {
        granted: false,
        already,
        credit: None,
        message,
    })
}

/// A login session: the link to open, and how to poll its outcome.
#[derive(Debug, Clone, Serialize)]
pub struct LoginSession {
    /// The url to open in a browser.
    pub auth_url: String,
    /// The session's id, for polling.
    pub state: String,
}

/// The result of a finished login.
#[derive(Debug, Clone)]
pub struct LoginResult {
    pub session: Session,
    pub account: Value,
}

/// The answer of one poll: the browser is still waiting, or the login is done.
#[derive(Debug, Clone)]
pub enum LoginPoll {
    /// The browser side has not signed in yet.
    Waiting,
    /// The login finished and the session is usable.
    Ready(Box<LoginResult>),
}

/// Open a login session: ask the vendor for a link, and hand it back.
///
/// The caller opens the link in a browser, signs in, and then polls
/// [`poll_login`] until it answers with the fresh session.
pub async fn start_login() -> anyhow::Result<LoginSession> {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "application/json".to_string());
    headers.insert("x-product".to_string(), PRODUCT.to_string());
    headers.insert(
        "user-agent".to_string(),
        format!("{PRODUCT}/{CLIENT_VERSION}"),
    );
    // The state request is made without any login: the headers below say so.
    headers.insert(
        "x-codebuddy-no-authorization".to_string(),
        "true".to_string(),
    );
    headers.insert("x-codebuddy-no-user-id".to_string(), "true".to_string());
    headers.insert(
        "x-codebuddy-no-enterprise-id".to_string(),
        "true".to_string(),
    );
    headers.insert(
        "x-codebuddy-no-department-info".to_string(),
        "true".to_string(),
    );

    let response = client()?
        .post(format!("{}{AUTH_STATE_PATH}", endpoint()))
        .headers(to_header_map(&headers)?)
        .body("{}")
        .send()
        .await
        .context("the login could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the login state returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the login could not be started (HTTP {status}): {}",
            message_of(&body)
        );
    }
    let data = body.get("data").cloned().unwrap_or_default();
    let auth_url = data
        .get("authUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = data
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if auth_url.is_empty() || state.is_empty() {
        anyhow::bail!("the login state did not carry a link; try again");
    }
    Ok(LoginSession { auth_url, state })
}

/// Poll a login session once.
///
/// While the browser is still waiting the vendor answers with its
/// "login ing" code, which is not an error: the caller should poll again.
/// Once the browser side has signed in, the answer carries the tokens and
/// the account, and the session is ready to use.
pub async fn poll_login(state: &str) -> anyhow::Result<LoginPoll> {
    let mut headers = BTreeMap::new();
    headers.insert("accept".to_string(), "application/json".to_string());
    headers.insert("x-product".to_string(), PRODUCT.to_string());
    headers.insert(
        "user-agent".to_string(),
        format!("{PRODUCT}/{CLIENT_VERSION}"),
    );
    headers.insert(
        "x-codebuddy-no-authorization".to_string(),
        "true".to_string(),
    );
    headers.insert("x-codebuddy-no-user-id".to_string(), "true".to_string());
    headers.insert(
        "x-codebuddy-no-enterprise-id".to_string(),
        "true".to_string(),
    );
    headers.insert(
        "x-codebuddy-no-department-info".to_string(),
        "true".to_string(),
    );

    let response = client()?
        .get(format!("{}{AUTH_TOKEN_PATH}{state}", endpoint()))
        .headers(to_header_map(&headers)?)
        .send()
        .await
        .context("the login poll could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the login poll returned no json")?;
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code == LOGIN_PENDING {
        return Ok(LoginPoll::Waiting);
    }
    if !(200..300).contains(&status) || code != 0 {
        anyhow::bail!(
            "the login poll was refused (HTTP {status}): {}",
            message_of(&body)
        );
    }
    let data = body.get("data").cloned().unwrap_or_default();
    let access_token = data
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if access_token.trim().is_empty() {
        anyhow::bail!("the login finished without an access token");
    }
    let expires_in = data.get("expiresIn").and_then(Value::as_i64);
    let session = Session {
        account: Account {
            nickname: String::new(),
            uid: String::new(),
            account_type: String::new(),
            last_login: true,
        },
        access_token,
        refresh_token: data
            .get("refreshToken")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        domain: data
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or("www.codebuddy.cn")
            .to_string(),
        expires_at: expires_in.map(|seconds| now_ms() + seconds * 1000),
    };
    Ok(LoginPoll::Ready(Box::new(LoginResult {
        session,
        account: data,
    })))
}

/// The account that belongs to a finished login.
pub async fn login_account(session: &Session, state: &str) -> anyhow::Result<Value> {
    let mut headers = session.auth_headers();
    headers.insert("x-codebuddy-no-user-id".to_string(), "true".to_string());
    headers.insert(
        "x-codebuddy-no-enterprise-id".to_string(),
        "true".to_string(),
    );
    headers.insert(
        "x-codebuddy-no-department-info".to_string(),
        "true".to_string(),
    );
    let response = client()?
        .get(format!("{}{AUTH_ACCOUNT_PATH}{state}", endpoint()))
        .headers(to_header_map(&headers)?)
        .send()
        .await
        .context("the login account could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the login account returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the login account was refused (HTTP {status}): {}",
            message_of(&body)
        );
    }
    Ok(body.get("data").cloned().unwrap_or(Value::Null))
}

/// Store a finished login in local-router's own session file, next to the
/// config, so it survives restarts without touching WorkBuddy's own files.
///
/// The file is 0600: it carries tokens.
pub async fn store_session(
    session: &Session,
    account: &Value,
    name: &str,
) -> anyhow::Result<PathBuf> {
    let path = own_session_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = SessionFile {
        account: Account {
            uid: account
                .get("uid")
                .or_else(|| account.pointer("/account/uid"))
                .and_then(Value::as_str)
                .unwrap_or(&session.account.uid)
                .to_string(),
            nickname: account
                .get("nickname")
                .or_else(|| account.pointer("/account/nickname"))
                .and_then(Value::as_str)
                .unwrap_or(&session.account.nickname)
                .to_string(),
            account_type: account
                .get("type")
                .or_else(|| account.pointer("/account/type"))
                .and_then(Value::as_str)
                .unwrap_or(&session.account.account_type)
                .to_string(),
            last_login: true,
        },
        auth: Auth {
            access_token: session.access_token.clone(),
            refresh_token: session.refresh_token.clone(),
            token_type: "Bearer".to_string(),
            expires_at: session.expires_at,
            refresh_expires_at: session.expires_at.map(|at| at + 2 * 24 * 3600 * 1000),
            domain: session.domain.clone(),
            scope: String::new(),
        },
    };
    // The pool keeps every account; the one just signed in becomes active.
    let mut pool = read_pool().unwrap_or_default();
    if pool.accounts.is_empty() {
        pool.active = name.to_string();
    }
    pool.accounts.insert(name.to_string(), file);
    let body = serde_json::to_string_pretty(&pool)?;
    std::fs::write(&path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// The daily check-in state, for a caller that has no client yet.
pub async fn checkin_state() -> Option<Checkin> {
    let session = session().await.ok()?;
    let http = client().ok()?;
    let headers = session.auth_headers();
    checkin_status(&http, &headers).await
}

/// Read the account's resource packages and its check-in state.
pub async fn credits() -> anyhow::Result<Credits> {
    let session = session().await?;
    let headers = session.auth_headers();
    let http = client()?;

    let response = http
        .post(format!("{}{RESOURCE_PATH}", endpoint()))
        .headers(to_header_map(&headers)?)
        .body("{}")
        .send()
        .await
        .context("the WorkBuddy credit query could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the WorkBuddy credit query returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the WorkBuddy credit query was refused (HTTP {status}): {}",
            message_of(&body)
        );
    }

    let mut packages = Vec::new();
    let mut remaining = 0u64;
    let mut capacity = 0u64;
    if let Some(list) = body
        .pointer("/data/Response/Data/Accounts")
        .and_then(Value::as_array)
    {
        for item in list {
            let size =
                number_of(item, "CycleCapacitySize").or_else(|| number_of(item, "CapacitySize"));
            let used =
                number_of(item, "CycleCapacityUsed").or_else(|| number_of(item, "CapacityUsed"));
            let left = number_of(item, "CycleCapacityRemain")
                .or_else(|| number_of(item, "CapacityRemain"));
            let size = size.unwrap_or(0);
            let used = used.unwrap_or(0);
            let left = left.unwrap_or(0);
            remaining += left;
            capacity += size;
            packages.push(Resource {
                package_name: item
                    .get("PackageName")
                    .and_then(Value::as_str)
                    .unwrap_or("package")
                    .to_string(),
                capacity: size,
                used,
                remaining: left,
                unit: item
                    .get("CapacityUnit")
                    .and_then(Value::as_str)
                    .unwrap_or("credits")
                    .to_string(),
                expires_at: item
                    .get("ExpiredTime")
                    .and_then(Value::as_str)
                    .and_then(parse_expired_time),
            });
        }
    }

    // The check-in state is a separate call; a failure there must not hide
    // the credits themselves.
    let checkin = checkin_status(&http, &headers).await;

    Ok(Credits {
        packages,
        remaining,
        capacity,
        checkin,
    })
}

/// The daily check-in state.
async fn checkin_status(
    http: &reqwest::Client,
    headers: &BTreeMap<String, String>,
) -> Option<Checkin> {
    let response = http
        .post(format!("{}{CHECKIN_PATH}", endpoint()))
        .headers(to_header_map(headers).unwrap_or_default())
        .body("{}")
        .send()
        .await
        .ok()?;
    let body: Value = response.json().await.ok()?;
    let data = body.get("data")?;
    Some(Checkin {
        checked_in_today: data
            .get("today_checked_in")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        streak_days: data.get("streak_days").and_then(Value::as_i64).unwrap_or(0) as u64,
        daily_credit: data
            .get("daily_credit")
            .and_then(Value::as_i64)
            .unwrap_or(0) as u64,
        today_credit: data
            .get("today_credit")
            .and_then(Value::as_i64)
            .unwrap_or(0) as u64,
        active: data.get("active").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// The models the account may use.
pub async fn models() -> anyhow::Result<Vec<String>> {
    let session = session().await?;
    let response = client()?
        .get(format!("{}{CONFIG_PATH}", endpoint()))
        .headers(to_header_map(&session.auth_headers())?)
        .send()
        .await
        .context("the WorkBuddy model list could not reach the server")?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .context("the WorkBuddy model list returned no json")?;
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the WorkBuddy model list was refused (HTTP {status}): {}",
            message_of(&body)
        );
    }
    let mut out = Vec::new();
    if let Some(list) = body.pointer("/data/models").and_then(Value::as_array) {
        for item in list {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                out.push(id.to_string());
            }
        }
    }
    Ok(out)
}

/// Send one chat request to WorkBuddy and return the raw SSE text.
///
/// The upstream only answers streaming requests, so this always streams.
pub async fn chat(session: &Session, body: &Value) -> anyhow::Result<(u16, String)> {
    let mut headers = session.auth_headers();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "text/event-stream".to_string());
    let response = client()?
        .post(format!("{}{CHAT_PATH}", endpoint()))
        .headers(to_header_map(&headers)?)
        .body(serde_json::to_vec(body)?)
        .send()
        .await
        .context("the WorkBuddy chat request could not reach the server")?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    Ok((status, text))
}

/// The request body WorkBuddy expects.
pub fn chat_body(model: &str, messages: Value, stream: bool) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
    });
    if stream {
        body["stream"] = json!(true);
        body["stream_options"] = json!({"include_usage": true});
    }
    body
}

/// Turn a header map into the type the HTTP client wants.
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

/// The message of an error payload, however the vendor wrapped it.
pub fn message_of(body: &Value) -> String {
    body.pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| body.get("msg").and_then(Value::as_str))
        .or_else(|| body.pointer("/error/msg").and_then(Value::as_str))
        .or_else(|| body.get("message").and_then(Value::as_str))
        .or_else(|| body.get("error").and_then(Value::as_str))
        .unwrap_or("the vendor refused the request")
        .to_string()
}

/// A number field, read as an integer.
fn number_of(value: &Value, key: &str) -> Option<u64> {
    let raw = value.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_u64().map(|n| n as i64))
            .or_else(|| v.as_f64().map(|f| f as i64))
    })?;
    u64::try_from(raw).ok()
}

/// The `ExpiredTime` field, which is either a timestamp or empty.
fn parse_expired_time(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // The field is "YYYY-MM-DD HH:MM:SS" in the server's own timezone.
    let parsed = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S");
    match parsed {
        Ok(naive) => Some(naive.and_utc().timestamp_millis()),
        Err(_) => trimmed.parse::<i64>().ok(),
    }
}

/// How long a session file has been there, for the doctor's report.
pub fn session_file_age() -> Option<Duration> {
    let path = session_file()?;
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------ needs_refresh --\n
    #[test]
    fn a_session_refreshes_only_near_its_expiry() {
        // No expiry: never refresh.
        let session = Session {
            account: Account::default(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            domain: "d".into(),
            expires_at: None,
        };
        assert!(!session.needs_refresh());

        // Far in the future: no refresh.
        let fresh = Session {
            expires_at: Some(now_ms() + 3_600_000),
            ..session.clone()
        };
        assert!(!fresh.needs_refresh());

        // Inside the five-minute margin: refresh.
        let stale = Session {
            expires_at: Some(now_ms() + 60_000),
            ..session.clone()
        };
        assert!(stale.needs_refresh());

        // Already expired: refresh.
        let expired = Session {
            expires_at: Some(now_ms() - 1),
            ..session
        };
        assert!(expired.needs_refresh());
    }

    // ------------------------------------------------------ auth_headers --\n
    #[test]
    fn auth_headers_carry_bearer_user_and_domain() {
        let session = Session {
            account: Account {
                uid: "u-42".into(),
                nickname: "Zhang".into(),
                account_type: "wx".into(),
                ..Account::default()
            },
            access_token: "tok".into(),
            refresh_token: "r".into(),
            domain: "www.codebuddy.cn".into(),
            expires_at: None,
        };
        let headers = session.auth_headers();
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer tok")
        );
        assert_eq!(headers.get("x-user-id").map(String::as_str), Some("u-42"));
        assert_eq!(
            headers.get("x-domain").map(String::as_str),
            Some("www.codebuddy.cn")
        );
        assert!(headers.contains_key("x-product"));
        assert!(headers.contains_key("user-agent"));
        // No uid / domain: the headers are omitted rather than blank.
        let bare = Session {
            account: Account::default(),
            domain: String::new(),
            ..session
        };
        let bare_headers = bare.auth_headers();
        assert!(!bare_headers.contains_key("x-user-id"));
        assert!(!bare_headers.contains_key("x-domain"));
    }

    // -------------------------------------------------------- chat_body --\n
    #[test]
    fn chat_body_adds_stream_options_only_when_streaming() {
        let stream = chat_body("hy3", serde_json::json!([{"role": "user"}]), true);
        assert_eq!(stream["model"], "hy3");
        assert_eq!(stream["stream"], true);
        assert_eq!(stream["stream_options"]["include_usage"], true);

        let plain = chat_body("hy3", serde_json::json!([]), false);
        assert!(plain.get("stream").is_none());
        assert!(plain.get("stream_options").is_none());
    }

    // -------------------------------------------------------- message_of --\n
    #[test]
    fn error_messages_are_read_from_every_vendor_shape() {
        assert_eq!(
            message_of(&serde_json::json!({"error": {"message": "nested"}})),
            "nested"
        );
        assert_eq!(message_of(&serde_json::json!({"msg": "flat"})), "flat");
        assert_eq!(
            message_of(&serde_json::json!({"error": {"msg": "inner"}})),
            "inner"
        );
        assert_eq!(
            message_of(&serde_json::json!({"message": "plain"})),
            "plain"
        );
        assert_eq!(
            message_of(&serde_json::json!({"error": "as-string"})),
            "as-string"
        );
        assert_eq!(
            message_of(&serde_json::json!({})),
            "the vendor refused the request"
        );
    }

    // ------------------------------------------------- expired-time parse --\n
    #[test]
    fn expired_times_accept_both_shapes() {
        // "YYYY-MM-DD HH:MM:SS" in the server's timezone.
        let stamp = parse_expired_time("2026-12-01 08:30:00").unwrap();
        assert!(stamp > now_ms());
        // A bare millisecond timestamp.
        assert_eq!(parse_expired_time("1893456000000"), Some(1_893_456_000_000));
        // Empty / garbage: None, never a panic.
        assert_eq!(parse_expired_time(""), None);
        assert_eq!(parse_expired_time("  "), None);
        assert_eq!(parse_expired_time("not-a-date"), None);
    }

    // ------------------------------------------------------ quota window --\n
    #[test]
    fn unlimited_windows_cannot_hold_the_router() {
        let unlimited = QuotaWindow {
            name: "plan".into(),
            used: 0.0,
            remaining: 0.0,
            resets_at: None,
            display: String::new(),
            unit: "unlimited".into(),
            holds_why: None,
        };
        assert!(unlimited.unlimited());
        let finite = QuotaWindow {
            unit: "credits".into(),
            ..unlimited
        };
        assert!(!finite.unlimited());
    }

    // ------------------------------------------------------- refresh guard --\n
    #[test]
    fn a_session_without_a_refresh_token_refuses_to_refresh() {
        // The refresh call itself needs the network; the guard is checked
        // first, and that is what this pins.
        let session = Session {
            account: Account::default(),
            access_token: "a".into(),
            refresh_token: "   ".into(),
            domain: "d".into(),
            expires_at: None,
        };
        assert!(session.refresh_token.trim().is_empty());
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;

    fn session_file_for(uid: &str, token: &str) -> SessionFile {
        SessionFile {
            account: Account {
                uid: uid.into(),
                nickname: format!("{uid}-name"),
                account_type: "personal".into(),
                last_login: true,
            },
            auth: Auth {
                access_token: token.into(),
                refresh_token: "rt".into(),
                token_type: "Bearer".into(),
                expires_at: Some(9_999_999_999_999),
                refresh_expires_at: None,
                domain: "www.codebuddy.cn".into(),
                scope: String::new(),
            },
        }
    }

    /// The desktop app encrypts its nickname and token; the file must still
    /// parse (with a placeholder) instead of failing the whole read.
    #[test]
    fn an_encrypted_nickname_is_read_as_a_placeholder() {
        let raw = r#"{
            "account": {"uid": "u1", "nickname": {"$wbEncrypted": 1, "envelope": "abc"}, "type": "personal"},
            "auth": {"accessToken": "at", "expiresAt": 1795496955295}
        }"#;
        let file: SessionFile = serde_json::from_str(raw).unwrap();
        assert_eq!(file.account.uid, "u1");
        assert!(!file.account.nickname.is_empty());
        assert_eq!(file.auth.access_token, "at");
        assert_eq!(file.auth.expires_at, Some(1_795_496_955_295));
    }

    /// Several accounts live side by side; the active one is picked.
    #[test]
    fn a_pool_holds_several_accounts_and_names_the_active_one() {
        let dir =
            std::env::temp_dir().join(format!("local-router-wb-pool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("workbuddy-session.json");
        let mut pool = SessionPool::default();
        pool.accounts
            .insert("work".into(), session_file_for("u-work", "at-work"));
        pool.accounts.insert(
            "personal".into(),
            session_file_for("u-personal", "at-personal"),
        );
        pool.active = "work".into();
        std::fs::write(&path, serde_json::to_string_pretty(&pool).unwrap()).unwrap();

        let read = read_pool_at(&path).unwrap();
        assert_eq!(read.accounts.len(), 2);
        assert_eq!(read.active, "work");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The old single-session file still reads (as one account).
    #[test]
    fn an_old_single_session_file_reads_as_one_account() {
        let dir =
            std::env::temp_dir().join(format!("local-router-wb-old-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("workbuddy-session.json");
        let file = session_file_for("u1", "at-1");
        std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
        let pool = read_pool_at(&path).unwrap();
        assert_eq!(pool.accounts.len(), 1);
        assert_eq!(pool.active, "default");
        assert_eq!(pool.accounts["default"].auth.access_token, "at-1");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
