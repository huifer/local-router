//! The on-disk configuration: providers, routing groups, settings and the
//! keys callers use to reach the gateway.
//!
//! Everything lives in one JSON file under `~/.config/local-router/` so that a
//! backup is a single copy and a restore is a single paste.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::protocol::ir::Protocol;

/// Where local-router keeps its files.
///
/// `$LOCAL_ROUTER_HOME` wins, then `$XDG_CONFIG_HOME/local-router`, then
/// `~/.config/local-router`.
pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LOCAL_ROUTER_HOME") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir).join("local-router");
        }
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".config").join("local-router");
    }
    PathBuf::from(".local-router")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

pub fn usage_file() -> PathBuf {
    config_dir().join("usage.jsonl")
}

/// Where a provider's media endpoints live.
///
/// Most vendors follow the OpenAI `/v1` family; others (WorkBuddy) keep
/// their media next to their chat endpoint. Anything left unset follows
/// the default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaPaths {
    /// Where an image is generated, e.g. `/v2/images/generations`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_generations: Option<String>,
    /// Where an image is edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_edits: Option<String>,
    /// Where a video task is created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_create: Option<String>,
    /// Where a video task's state is read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_status: Option<String>,
    /// Where the finished video is downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_content: Option<String>,
    /// Where a video task is cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_delete: Option<String>,
}

/// A vendor key local-router holds for one provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderKey {
    #[serde(default = "new_id")]
    pub id: String,
    /// The secret. Never sent to a browser.
    pub key: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// A friendly name, e.g. which team card this is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Where a provider's credential comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthKind {
    /// A key stored in this file. The default.
    Stored,
    /// The WorkBuddy app's own login session, read from the machine.
    #[serde(rename = "workbuddy")]
    WorkBuddy,
}

fn yes() -> bool {
    true
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

impl ProviderKey {
    pub fn new(key: impl Into<String>) -> Self {
        ProviderKey {
            id: new_id(),
            key: key.into(),
            enabled: true,
            label: None,
        }
    }
}

/// One vendor local-router can talk to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    /// The id used in `provider/model` names, e.g. `deepseek`.
    pub id: String,
    /// The name shown in the web UI.
    pub name: String,
    /// The preset this came from, or `custom`.
    #[serde(default = "custom_kind")]
    pub kind: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Which wire protocol this vendor speaks.
    #[serde(default = "default_protocol")]
    pub protocol: Protocol,
    /// The base URL of an OpenAI-compatible API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The base URL of an Anthropic-compatible API, when different.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_base_url: Option<String>,
    /// The base URL of an OpenAI Responses API, when different.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses_base_url: Option<String>,
    /// The base URL of a Gemini API, when different.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_base_url: Option<String>,
    #[serde(default)]
    pub keys: Vec<ProviderKey>,
    /// The model ids local-router exposes for this provider.
    #[serde(default)]
    pub models: Vec<String>,
    /// Glob patterns: a newly listed model matching one is exposed.
    #[serde(default)]
    pub auto_include: Vec<String>,
    /// The models.dev catalog this provider borrows its list from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// Extra headers sent on every request to this provider.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Send the calling agent's own User-Agent instead of local-router's.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// An HTTP proxy for this provider's requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// The provider is only reachable through a routing group.
    #[serde(default)]
    pub group_only: bool,
    /// Where the credential comes from. Absent means a stored key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthKind>,
    /// The path of the chat endpoint, when the vendor does not use the
    /// protocol's default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_path: Option<String>,
    /// The upstream only answers streaming requests. The gateway asks it
    /// for a stream and assembles a whole reply when the caller wants one.
    #[serde(default)]
    pub stream_only: bool,
    /// Stop sending requests here while the account's quota window says it
    /// is empty. Off by default: some accounts keep serving on an empty
    /// window, and holding them would break working requests.
    #[serde(default)]
    pub hold_when_exhausted: bool,
    /// The models this provider offers for image and video work.
    #[serde(default)]
    pub media_models: Vec<String>,
    /// The provider's native media endpoints, when they are not the
    /// OpenAI `/v1` family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_paths: Option<MediaPaths>,
}

impl Default for Provider {
    fn default() -> Self {
        Provider {
            id: String::new(),
            name: String::new(),
            kind: "custom".to_string(),
            enabled: true,
            protocol: Protocol::OpenAiChat,
            base_url: None,
            anthropic_base_url: None,
            responses_base_url: None,
            gemini_base_url: None,
            keys: Vec::new(),
            models: Vec::new(),
            auto_include: Vec::new(),
            catalog: None,
            headers: BTreeMap::new(),
            user_agent: None,
            proxy: None,
            group_only: false,
            auth: None,
            chat_path: None,
            stream_only: false,
            hold_when_exhausted: false,
            media_models: Vec::new(),
            media_paths: None,
        }
    }
}

impl Provider {
    /// Whether this provider authenticates with a local app's session
    /// rather than a key stored in the config.
    pub fn uses_session(&self) -> bool {
        self.auth == Some(AuthKind::WorkBuddy)
    }
}

fn custom_kind() -> String {
    "custom".to_string()
}

fn default_protocol() -> Protocol {
    Protocol::OpenAiChat
}

impl Provider {
    pub fn enabled_keys(&self) -> impl Iterator<Item = &ProviderKey> {
        self.keys
            .iter()
            .filter(|k| k.enabled && !k.key.trim().is_empty())
    }

    /// The base URL to use for a given protocol.
    pub fn base_url_for(&self, protocol: Protocol) -> Option<String> {
        let specific = match protocol {
            Protocol::OpenAiChat => self.base_url.clone(),
            Protocol::Anthropic => self.anthropic_base_url.clone().or(self.base_url.clone()),
            Protocol::OpenAiResponses => self.responses_base_url.clone().or(self.base_url.clone()),
            Protocol::Gemini => self.gemini_base_url.clone().or(self.base_url.clone()),
        };
        specific.filter(|u| !u.trim().is_empty())
    }
}

/// How a group picks between its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Routing {
    /// The member whose quota renews soonest goes first.
    #[default]
    Smart,
    /// The first member until it cannot answer, then the next.
    Order,
    /// Each turn to the next member.
    Rotate,
    /// The least used member first.
    Usage,
    /// Each member takes its share, smoothly interleaved.
    Weight,
}

impl Routing {
    pub fn as_str(&self) -> &'static str {
        match self {
            Routing::Smart => "smart",
            Routing::Order => "order",
            Routing::Rotate => "rotate",
            Routing::Usage => "usage",
            Routing::Weight => "weight",
        }
    }
}

/// How long a conversation stays with the member that answered it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Stays {
    /// While the vendor's prompt cache is worth keeping.
    #[default]
    Auto,
    /// For the whole session.
    Session,
    /// For the turn.
    Turn,
    /// Never move.
    Off,
    /// Never move, and never fall back to another member.
    Strict,
}

impl Stays {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stays::Auto => "auto",
            Stays::Session => "session",
            Stays::Turn => "turn",
            Stays::Off => "off",
            Stays::Strict => "strict",
        }
    }
}

/// Several models an agent picks as one: `group/<id>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// The members, as `provider/model` or `group/id`.
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub routing: Routing,
    /// The weight of each member, when routing is `weight`.
    #[serde(default)]
    pub weights: BTreeMap<String, u32>,
    #[serde(default)]
    pub stays: Stays,
    /// The models the group names stay out of the global model list.
    #[serde(default = "yes")]
    pub listed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A key other machines use to call the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerKey {
    pub id: String,
    pub name: String,
    pub key: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// A daily token cap, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_cost: Option<f64>,
}

/// The settings the web UI exposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The port the gateway listens on; `LOCAL_ROUTER_GATEWAY_PORT` wins.
    pub gateway_port: u16,
    /// The port the web UI listens on.
    pub web_port: u16,
    /// Bind on the local network as well.
    pub lan: bool,
    /// The address the gateway binds to.
    pub gateway_addr: String,
    pub web_addr: String,
    /// The key callers use when they send none.
    pub default_caller_key: String,
    /// Send the agent's own User-Agent to providers that ask for it.
    pub send_agent_user_agent: bool,
    /// Mask secrets in prompts before they leave the machine.
    pub masking: bool,
    /// Keep a record of every request.
    pub record_usage: bool,
    /// The user's locale for the web UI.
    pub locale: String,
    /// Origins a browser may call the gateway from. Empty means none:
    /// the gateway is for local tools, and a browser on another site
    /// should not be able to use it. `["*"]` allows any origin.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Claim the WorkBuddy account's daily check-in on its own, so the
    /// day's credits land without anyone opening the app.
    #[serde(default = "yes")]
    pub workbuddy_auto_checkin: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            gateway_port: 3425,
            web_port: 3426,
            lan: false,
            gateway_addr: "127.0.0.1".to_string(),
            web_addr: "127.0.0.1".to_string(),
            default_caller_key: "local-router".to_string(),
            send_agent_user_agent: false,
            masking: false,
            record_usage: true,
            locale: "zh-CN".to_string(),
            cors_origins: Vec::new(),
            workbuddy_auto_checkin: true,
        }
    }
}

impl Settings {
    /// Apply the `LOCAL_ROUTER_*` environment variables over the file.
    ///
    /// The file stays untouched; this only bends what a process listens on,
    /// which is what a container or a second instance needs.
    pub fn from_env(mut self) -> Self {
        let mut applied = false;
        if let Ok(port) = std::env::var("LOCAL_ROUTER_GATEWAY_PORT") {
            if let Ok(port) = port.trim().parse() {
                self.gateway_port = port;
                applied = true;
            }
        }
        if let Ok(port) = std::env::var("LOCAL_ROUTER_WEB_PORT") {
            if let Ok(port) = port.trim().parse() {
                self.web_port = port;
                applied = true;
            }
        }
        if let Ok(addr) = std::env::var("LOCAL_ROUTER_GATEWAY_ADDR") {
            if !addr.trim().is_empty() {
                self.gateway_addr = addr.trim().to_string();
                applied = true;
            }
        }
        if let Ok(addr) = std::env::var("LOCAL_ROUTER_WEB_ADDR") {
            if !addr.trim().is_empty() {
                self.web_addr = addr.trim().to_string();
                applied = true;
            }
        }
        if let Ok(key) = std::env::var("LOCAL_ROUTER_DEFAULT_KEY") {
            if !key.trim().is_empty() {
                self.default_caller_key = key.trim().to_string();
                applied = true;
            }
        }
        if let Ok(lan) = std::env::var("LOCAL_ROUTER_LAN") {
            if matches!(lan.trim(), "1" | "true" | "yes") {
                self.lan = true;
                self.gateway_addr = "0.0.0.0".to_string();
                self.web_addr = "0.0.0.0".to_string();
                applied = true;
            }
        }
        if applied {
            tracing::info!("the LOCAL_ROUTER_* environment overrode the file");
        }
        self
    }
}

/// The whole configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub providers: Vec<Provider>,
    pub groups: Vec<Group>,
    pub settings: Settings,
    pub caller_keys: Vec<CallerKey>,
}

impl Config {
    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn provider_mut(&mut self, id: &str) -> Option<&mut Provider> {
        self.providers.iter_mut().find(|p| p.id == id)
    }

    pub fn group(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_mut(&mut self, id: &str) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Every model the gateway serves, as `provider/model` or `group/id`.
    pub fn model_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        for p in &self.providers {
            if !p.enabled {
                continue;
            }
            for m in &p.models {
                out.push(format!("{}/{}", p.id, m));
            }
        }
        for g in &self.groups {
            if g.listed {
                out.push(format!("group/{}", g.id));
            }
        }
        out
    }

    /// Split a `provider/model` id.
    pub fn split_model(id: &str) -> Option<(&str, &str)> {
        let (head, tail) = id.split_once('/')?;
        if head.is_empty() || tail.is_empty() {
            return None;
        }
        Some((head, tail))
    }
}

/// Reads and writes the configuration file.
pub struct Store {
    path: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    pub fn new() -> Self {
        Store {
            path: config_file(),
        }
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Store { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where the usage log lives: next to the config file it belongs to.
    pub fn usage_path(&self) -> PathBuf {
        self.path
            .parent()
            .map(|dir| dir.join("usage.jsonl"))
            .unwrap_or_else(|| config_dir().join("usage.jsonl"))
    }

    pub fn load(&self) -> anyhow::Result<Config> {
        if !self.path.exists() {
            return Ok(Config::default());
        }
        let raw = std::fs::read_to_string(&self.path)?;
        if raw.trim().is_empty() {
            return Ok(Config::default());
        }
        match serde_json::from_str::<Config>(&raw) {
            Ok(cfg) => Ok(cfg),
            Err(err) => {
                tracing::warn!("config at {} is not valid json: {err}", self.path.display());
                Ok(Config::default())
            }
        }
    }

    pub fn save(&self, config: &Config) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(config)?;
        std::fs::write(&tmp, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// When the file was last written, so a running local-router can tell whether
    /// another one — the CLI, say — changed it.
    pub fn mtime(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
    }
}

/// The shared, mutable state the server hands to every handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<tokio::sync::RwLock<Config>>,
    pub store: Arc<Store>,
    pub started: chrono::DateTime<chrono::Utc>,
    /// The address the web page actually listens on. Startup overlays the
    /// environment and the command line on the file; handlers that care —
    /// the console's remote guard — read this, not the file.
    pub web_bind: Arc<tokio::sync::RwLock<String>>,
    /// The file's mtime the last time it was read, so a change made by the
    /// CLI is picked up without a restart.
    mtime: Arc<tokio::sync::Mutex<Option<std::time::SystemTime>>>,
}

impl AppState {
    pub async fn load(store: Arc<Store>) -> anyhow::Result<Self> {
        let config = store.load()?;
        let mtime = store.mtime();
        let web_bind = config.settings.web_addr.clone();
        Ok(AppState {
            config: Arc::new(tokio::sync::RwLock::new(config)),
            store,
            started: chrono::Utc::now(),
            web_bind: Arc::new(tokio::sync::RwLock::new(web_bind)),
            mtime: Arc::new(tokio::sync::Mutex::new(mtime)),
        })
    }

    /// Record where the web page really listens, after the environment and
    /// the command line have had their say.
    pub async fn set_web_bind(&self, addr: &str) {
        *self.web_bind.write().await = addr.to_string();
    }

    /// Where the web page is actually listening right now.
    pub async fn web_bind(&self) -> String {
        self.web_bind.read().await.clone()
    }

    /// Where the usage log of this configuration is written.
    pub fn usage_path(&self) -> PathBuf {
        self.store.usage_path()
    }

    /// Pick up changes another local-router made to the file.
    pub async fn sync(&self) {
        let fresh = self.store.mtime();
        let mut seen = self.mtime.lock().await;
        if *seen != fresh {
            *seen = fresh;
            if let Ok(config) = self.store.load() {
                *self.config.write().await = config;
                tracing::info!("the configuration changed on disk; reloaded it");
            }
        }
    }

    /// Apply a change to the configuration and write it out.
    pub async fn update<T>(&self, f: impl FnOnce(&mut Config) -> T) -> anyhow::Result<T> {
        let mut guard = self.config.write().await;
        let out = f(&mut guard);
        self.store.save(&guard)?;
        drop(guard);
        *self.mtime.lock().await = self.store.mtime();
        Ok(out)
    }
}
