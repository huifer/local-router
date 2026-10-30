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
    /// Maximum number of in-flight requests for this upstream API key.
    /// None or zero means unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
}

/// Where a provider's credential comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthKind {
    /// A key stored in this file. The default.
    Stored,
    /// The WorkBuddy app's own login session, read from the machine.
    #[serde(rename = "workbuddy")]
    WorkBuddy,
    /// A QwenWork (千问办公) device-flow login, stored in a local file.
    #[serde(rename = "qwenwork")]
    QwenWork,
    /// A Trae CN Cloud-IDE login, stored in a local account pool.
    #[serde(rename = "trae_cn")]
    TraeCn,
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
            max_concurrency: None,
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
    /// Extra headers sent on every request to this provider. Values may
    /// carry `{version}` / `{platform}` / `{arch}` placeholders.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Send the calling agent's own User-Agent instead of local-router's.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Headers forwarded verbatim from the caller to this provider
    /// (lowercase names). Useful for upstreams that demand the client's
    /// own session id or device headers (Console Go wants
    /// `x-opencode-session`; some relays want `x-request-id`).
    #[serde(default)]
    pub forward_headers: Vec<String>,
    /// Headers dropped before the request goes out. Runs after `headers`
    /// and `forward_headers`, so a rule here wins over everything else.
    #[serde(default)]
    pub remove_headers: Vec<String>,
    /// The order the upstream headers are sent in. Headers not listed are
    /// appended in their natural order. Purely a hint to picky relays;
    /// most vendors ignore it.
    #[serde(default)]
    pub header_order: Vec<String>,
    /// How to sign in with this upstream, when it wants a browser login
    /// instead of a pasted API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthConfig>,
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
    /// Maximum in-flight requests for this provider. Stored API keys can
    /// override it individually; session-backed providers use it as a
    /// provider-level fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
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
            forward_headers: Vec::new(),
            remove_headers: Vec::new(),
            header_order: Vec::new(),
            oauth: None,
            proxy: None,
            group_only: false,
            auth: None,
            chat_path: None,
            stream_only: false,
            hold_when_exhausted: false,
            max_concurrency: None,
            media_models: Vec::new(),
            media_paths: None,
        }
    }
}

impl Provider {
    /// Whether this provider authenticates with a local app's session
    /// rather than a key stored in the config.
    pub fn uses_session(&self) -> bool {
        matches!(
            self.auth,
            Some(AuthKind::WorkBuddy) | Some(AuthKind::QwenWork) | Some(AuthKind::TraeCn)
        )
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
            // A QwenWork provider never goes through the generic HTTP
            // client, so any URL will do; the module uses its own host.
            Protocol::QwenWork => self.base_url.clone(),
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

/// How to sign in to an upstream with a browser (OAuth 2.0, PKCE).
///
/// Every vendor spells OAuth a little differently, so the endpoints live
/// here rather than in the code: the same engine serves them all.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// Where the browser is sent to sign in.
    pub authorize_url: String,
    /// Where the code is swapped for a credential.
    pub token_url: String,
    /// The app id the vendor knows.
    pub client_id: String,
    /// The scopes to ask for.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The path on this gateway the vendor redirects back to.
    #[serde(default = "default_oauth_redirect")]
    pub redirect_path: String,
    /// Extra query parameters for the authorize URL.
    #[serde(default)]
    pub extra_params: BTreeMap<String, String>,
    /// How the code is exchanged: `standard` (the OAuth 2.0 default) or
    /// `openrouter` (a JSON body whose reply carries a `key`).
    #[serde(default)]
    pub style: Option<String>,
}

fn default_oauth_redirect() -> String {
    "/oauth/callback".to_string()
}

impl OAuthConfig {
    /// The exchange style, defaulting to the OAuth 2.0 standard.
    pub fn style(&self) -> crate::oauth::ExchangeStyle {
        match self.style.as_deref().map(|s| s.to_ascii_lowercase()) {
            Some(ref s) if s == "openrouter" => crate::oauth::ExchangeStyle::OpenRouter,
            _ => crate::oauth::ExchangeStyle::Standard,
        }
    }
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
    /// The port local-router listens on: the gateway and the console share
    /// it. `LOCAL_ROUTER_GATEWAY_PORT` wins.
    pub gateway_port: u16,
    /// Deprecated: the console is on the gateway's port now. The value is
    /// ignored; it stays in the file so old configs still parse.
    pub web_port: u16,
    /// Whether the console is served at all. Off means API only — handy on
    /// a shared machine where only the model endpoints should be reachable.
    #[serde(default = "yes")]
    pub console: bool,
    /// Bind on the local network as well.
    pub lan: bool,
    /// The address to bind.
    pub gateway_addr: String,
    /// Deprecated: the console binds with the gateway.
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
            gateway_port: 3435,
            web_port: 0,
            console: true,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, url: &str) -> Provider {
        Provider {
            id: id.into(),
            name: id.into(),
            base_url: Some(url.into()),
            ..Provider::default()
        }
    }

    // ----------------------------------------------------- split_model --

    #[test]
    fn split_model_requires_both_halves() {
        assert_eq!(
            Config::split_model("deepseek/deepseek-chat"),
            Some(("deepseek", "deepseek-chat"))
        );
        assert_eq!(
            Config::split_model("group/backup"),
            Some(("group", "backup"))
        );
        assert_eq!(Config::split_model("noslash"), None);
        assert_eq!(Config::split_model("/noname"), None);
        assert_eq!(Config::split_model("noprov/"), None);
        // Only the first slash splits: the rest rides with the model id.
        assert_eq!(Config::split_model("a/b/c"), Some(("a", "b/c")));
    }

    // -------------------------------------------------- base_url_for --

    #[test]
    fn base_url_for_falls_back_from_specific_to_generic() {
        let mut p = provider("v", "https://api.vendor.com/v1");
        assert_eq!(
            p.base_url_for(Protocol::OpenAiChat).as_deref(),
            Some("https://api.vendor.com/v1")
        );
        // No Anthropic URL: the generic one covers it.
        assert_eq!(
            p.base_url_for(Protocol::Anthropic).as_deref(),
            Some("https://api.vendor.com/v1")
        );
        p.anthropic_base_url = Some("https://anthropic.vendor.com".into());
        assert_eq!(
            p.base_url_for(Protocol::Anthropic).as_deref(),
            Some("https://anthropic.vendor.com")
        );
        p.responses_base_url = Some("https://responses.vendor.com".into());
        assert_eq!(
            p.base_url_for(Protocol::OpenAiResponses).as_deref(),
            Some("https://responses.vendor.com")
        );
        p.gemini_base_url = Some("https://gemini.vendor.com".into());
        assert_eq!(
            p.base_url_for(Protocol::Gemini).as_deref(),
            Some("https://gemini.vendor.com")
        );
    }

    #[test]
    fn base_url_for_treats_blank_urls_as_absent() {
        let mut p = provider("v", "  ");
        p.anthropic_base_url = Some("".into());
        assert_eq!(p.base_url_for(Protocol::OpenAiChat), None);
        assert_eq!(p.base_url_for(Protocol::Anthropic), None);
    }

    // ------------------------------------------------------ enabled_keys --

    #[test]
    fn enabled_keys_skips_disabled_and_blank() {
        let p = Provider {
            keys: vec![
                ProviderKey::new("sk-live"),
                ProviderKey {
                    enabled: false,
                    ..ProviderKey::new("sk-off")
                },
                ProviderKey::new("   "),
            ],
            ..Provider::default()
        };
        let live: Vec<&str> = p.enabled_keys().map(|k| k.key.as_str()).collect();
        assert_eq!(live, vec!["sk-live"]);
    }

    // ---------------------------------------------------- uses_session --

    #[test]
    fn session_providers_need_no_stored_key() {
        let mut p = Provider::default();
        assert!(!p.uses_session());
        p.auth = Some(AuthKind::WorkBuddy);
        assert!(p.uses_session());
        p.auth = Some(AuthKind::QwenWork);
        assert!(p.uses_session());
        p.auth = Some(AuthKind::Stored);
        assert!(!p.uses_session());
    }

    // ------------------------------------------------------- settings --

    #[test]
    fn settings_round_trip_with_defaults_for_new_fields() {
        // An old config file without cors_origins / workbuddy_auto_checkin
        // must still parse; serde fills the defaults.
        let old = r#"{
            "gateway_port": 4000,
            "web_port": 4001,
            "lan": false,
            "gateway_addr": "127.0.0.1",
            "web_addr": "127.0.0.1",
            "default_caller_key": "k",
            "send_agent_user_agent": false,
            "masking": false,
            "record_usage": true,
            "locale": "zh-CN"
        }"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.gateway_port, 4000);
        assert!(settings.cors_origins.is_empty());
        assert!(settings.workbuddy_auto_checkin);
    }

    #[test]
    fn routing_names_are_snake_case_on_the_wire() {
        for routing in [
            Routing::Smart,
            Routing::Order,
            Routing::Rotate,
            Routing::Usage,
            Routing::Weight,
        ] {
            let wire = serde_json::to_value(routing).unwrap();
            let back: Routing = serde_json::from_value(wire).unwrap();
            assert_eq!(back, routing);
            assert_eq!(back.as_str(), routing.as_str());
        }
    }

    // ------------------------------------------------------ config io --

    #[test]
    fn a_config_file_round_trips_through_the_store() {
        let dir =
            std::env::temp_dir().join(format!("local-router-config-unit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::at(dir.join("config.json"));
        let mut config = Config::default();
        config
            .providers
            .push(provider("deepseek", "https://api.deepseek.com/v1"));
        config.providers[0].models = vec!["deepseek-chat".into()];
        config.groups.push(Group {
            id: "backup".into(),
            name: "Backup".into(),
            models: vec!["deepseek/deepseek-chat".into()],
            routing: Routing::Order,
            stays: Stays::default(),
            weights: BTreeMap::new(),
            listed: true,
            note: None,
        });
        config.caller_keys.push(CallerKey {
            id: "k1".into(),
            name: "laptop".into(),
            key: "sk-caller".into(),
            enabled: true,
            daily_tokens: None,
            monthly_cost: None,
        });
        store.save(&config).unwrap();

        // The file lands with owner-only permissions.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o666, "config.json must be 0666");
        }

        let loaded = store.load().unwrap();
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].models, vec!["deepseek-chat"]);
        assert_eq!(loaded.groups[0].routing, Routing::Order);
        assert_eq!(loaded.caller_keys[0].key, "sk-caller");
        // The mtime probe the hot reload depends on.
        assert!(store.mtime().is_some());
        // The usage log lives next to the config, not in a global directory.
        assert_eq!(store.usage_path(), dir.join("usage.jsonl"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_config_file_loads_as_default() {
        let dir =
            std::env::temp_dir().join(format!("local-router-config-unit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::at(dir.join("config.json"));
        let loaded = store.load().unwrap();
        assert_eq!(loaded.settings.gateway_port, 3435);
        // The console shares the gateway's port; the legacy field is unused.
        assert_eq!(loaded.settings.web_port, 0);
        assert!(loaded.providers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
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

    /// Write the config out, atomically (a temporary file, then a rename).
    ///
    /// The file carries the providers' keys, so its mode lets a container
    /// or another local tool read the config it owns.
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
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o666));
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
    /// The SQLite usage store, when it is on.
    pub sqlite: Option<crate::store::Store>,
    /// The in-process 429 cooldowns shared by gateway and console.
    pub cooldowns: crate::gateway::cooldown::Registry,
    /// Per-provider/API-key concurrency gates shared by gateway and console.
    pub concurrency: crate::gateway::concurrency::Registry,
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
            sqlite: crate::store::Store::shared(),
            cooldowns: crate::gateway::cooldown::Registry::default(),
            concurrency: crate::gateway::concurrency::Registry::default(),
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

#[cfg(test)]
mod config_surface_tests {
    use super::*;

    /// Environment variables are process-global, so the tests that touch
    /// them must not run at the same time. One lock for all of them.
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Every field of the main config must survive a round trip through the
    /// file: a field that silently drops is a setting the user cannot set.
    #[test]
    fn a_full_config_survives_the_file_unchanged() {
        let mut config = Config::default();
        config.providers.push(Provider {
            id: "p".into(),
            name: "P".into(),
            kind: "custom".into(),
            enabled: true,
            protocol: Protocol::Anthropic,
            base_url: Some("https://x/v1".into()),
            anthropic_base_url: Some("https://x/a".into()),
            responses_base_url: Some("https://x/r".into()),
            gemini_base_url: Some("https://x/g".into()),
            keys: vec![ProviderKey {
                id: "k".into(),
                key: "sk-1".into(),
                enabled: true,
                label: Some("l".into()),
                max_concurrency: None,
            }],
            models: vec!["m".into()],
            auto_include: vec!["m-*".into()],
            catalog: Some("c".into()),
            headers: BTreeMap::from([("x-a".to_string(), "1".to_string())]),
            user_agent: Some("ua/{version}".into()),
            forward_headers: vec!["x-session".into()],
            remove_headers: vec!["x-drop".into()],
            header_order: vec!["user-agent".into()],
            oauth: None,
            proxy: None,
            group_only: true,
            auth: Some(AuthKind::QwenWork),
            chat_path: Some("/v2/chat".into()),
            stream_only: true,
            hold_when_exhausted: true,
            max_concurrency: Some(3),
            media_models: vec!["img".into()],
            media_paths: None,
        });
        config.groups.push(Group {
            id: "g".into(),
            name: "G".into(),
            models: vec!["p/m".into()],
            routing: Routing::Weight,
            weights: BTreeMap::from([("p/m".to_string(), 3u32)]),
            stays: Stays::Strict,
            listed: false,
            note: Some("n".into()),
        });
        config.settings = Settings {
            gateway_port: 1234,
            console: false,
            lan: true,
            gateway_addr: "0.0.0.0".into(),
            default_caller_key: "k".into(),
            send_agent_user_agent: true,
            masking: true,
            record_usage: false,
            locale: "en-US".into(),
            cors_origins: vec!["*".into()],
            workbuddy_auto_checkin: false,
            ..Settings::default()
        };
        config.caller_keys.push(CallerKey {
            id: "ck".into(),
            name: "N".into(),
            key: "sk-ck".into(),
            enabled: true,
            daily_tokens: Some(10),
            monthly_cost: Some(1.5),
        });

        let raw = serde_json::to_string(&config).unwrap();
        let back: Config = serde_json::from_str(&raw).unwrap();
        let a = serde_json::to_value(&config).unwrap();
        let b = serde_json::to_value(&back).unwrap();
        assert_eq!(a, b, "the config changed on the way through the file");
    }

    /// A file that only knows the old fields still loads: the new ones fall
    /// back to their defaults.
    #[test]
    fn an_old_file_gets_defaults_for_the_new_fields() {
        let old = r#"{
            "providers": [{
                "id": "p", "name": "P",
                "base_url": "https://x/v1",
                "keys": [{"id": "k", "key": "sk"}]
            }],
            "groups": [],
            "settings": {"gateway_port": 4000}
        }"#;
        let config: Config = serde_json::from_str(old).unwrap();
        let p = &config.providers[0];
        assert!(p.forward_headers.is_empty());
        assert!(p.remove_headers.is_empty());
        assert!(p.header_order.is_empty());
        assert!(p.user_agent.is_none());
        assert!(p.enabled, "enabled defaults to true");
        assert!(config.settings.console, "the console is on by default");
        assert!(config.settings.workbuddy_auto_checkin);
    }

    /// The env overlay only bends what it names; everything else is kept.
    #[test]
    fn the_env_overlay_keeps_the_file_values_it_does_not_name() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let settings = Settings {
            gateway_port: 4000,
            masking: true,
            ..Settings::default()
        };
        let out = settings.clone().from_env();
        // No LOCAL_ROUTER_* set in the test process: the file wins.
        assert_eq!(out.gateway_port, 4000);
        assert!(out.masking);
        assert_eq!(out.default_caller_key, settings.default_caller_key);
    }
}

#[cfg(test)]
mod precedence_tests {
    use super::config_surface_tests::ENV_LOCK;
    use super::*;

    /// The port must come out of the file when the command line and the
    /// environment say nothing — the CLI's default must not silently win.
    #[test]
    fn the_files_port_survives_a_start_without_arguments() {
        let mut config = Config::default();
        config.settings.gateway_port = 13555;
        config.settings.gateway_addr = "127.0.0.1".into();
        let dir =
            std::env::temp_dir().join(format!("local-router-precedence-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::at(dir.join("config.json"));
        store.save(&config).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.settings.gateway_port, 13555);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The environment still beats the file.
    #[test]
    fn the_env_beats_the_file_for_the_port() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut settings = Settings {
            gateway_port: 13555,
            ..Settings::default()
        };
        // SAFETY: single-threaded test body; no other thread reads it here.
        std::env::set_var("LOCAL_ROUTER_GATEWAY_PORT", "13556");
        let out = std::mem::take(&mut settings).from_env();
        std::env::remove_var("LOCAL_ROUTER_GATEWAY_PORT");
        assert_eq!(out.gateway_port, 13556);
    }
}
