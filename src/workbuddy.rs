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
    #[serde(default)]
    pub nickname: String,
    #[serde(default, rename = "type")]
    pub account_type: String,
    #[serde(default, rename = "lastLogin")]
    pub last_login: bool,
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
    let path = session_file().ok_or_else(|| {
        anyhow::anyhow!("the WorkBuddy login file was not found; open WorkBuddy and sign in first")
    })?;
    let raw = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "the WorkBuddy login file at {} is unreadable",
            path.display()
        )
    })?;
    let file: SessionFile = serde_json::from_str(&raw).with_context(|| {
        format!(
            "the WorkBuddy login file at {} is not valid json",
            path.display()
        )
    })?;
    if file.auth.access_token.trim().is_empty() {
        anyhow::bail!(
            "the WorkBuddy login file at {} holds no access token; sign in to WorkBuddy again",
            path.display()
        );
    }
    Ok(Session {
        account: file.account,
        access_token: file.auth.access_token,
        refresh_token: file.auth.refresh_token,
        domain: file.auth.domain,
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
    let current = read_session_file()?;
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
pub async fn store_session(session: &Session, account: &Value) -> anyhow::Result<PathBuf> {
    let path = crate::config::config_dir().join("workbuddy-session.json");
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
    let body = serde_json::to_string_pretty(&file)?;
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
