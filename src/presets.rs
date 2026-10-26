//! The vendors local-router knows out of the box.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::config::{AuthKind, Provider, ProviderKey};
use crate::protocol::ir::Protocol;

const RAW: &str = include_str!("presets.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    /// `vendor`, `relay` or `local`.
    pub group: String,
    pub protocol: Protocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(default)]
    pub default_models: Vec<String>,
    #[serde(default)]
    pub key_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    /// The provider authenticates with a local app's session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthKind>,
    /// The path of the chat endpoint, when the vendor does not use the
    /// protocol's default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_path: Option<String>,
    /// The upstream only answers streaming requests.
    #[serde(default)]
    pub stream_only: bool,
    /// The models this vendor offers for image and video work.
    #[serde(default)]
    pub media_models: Vec<String>,
    /// The vendor's native media endpoints, when they are not the OpenAI
    /// `/v1` family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_paths: Option<crate::config::MediaPaths>,
}

#[derive(Debug, Clone, Deserialize)]
struct PresetFile {
    presets: Vec<Preset>,
}

fn file() -> &'static PresetFile {
    static FILE: OnceLock<PresetFile> = OnceLock::new();
    FILE.get_or_init(|| {
        serde_json::from_str(RAW).expect("the built-in preset list must be valid json")
    })
}

pub fn all() -> &'static [Preset] {
    &file().presets
}

pub fn get(id: &str) -> Option<&'static Preset> {
    file().presets.iter().find(|p| p.id == id)
}

/// The presets, grouped the way the add sheet shows them.
pub fn grouped() -> BTreeMap<String, Vec<&'static Preset>> {
    let mut out: BTreeMap<String, Vec<&'static Preset>> = BTreeMap::new();
    for p in all() {
        out.entry(p.group.clone()).or_default().push(p);
    }
    out
}

/// Turn a preset into a provider the user can use right away.
pub fn to_provider(preset: &Preset, key: Option<&str>) -> Provider {
    Provider {
        id: preset.id.clone(),
        name: preset.name.clone(),
        kind: preset.id.clone(),
        enabled: true,
        protocol: preset.protocol,
        base_url: preset.base_url.clone(),
        anthropic_base_url: preset.anthropic_base_url.clone(),
        responses_base_url: None,
        gemini_base_url: preset.gemini_base_url.clone(),
        keys: key
            .filter(|k| !k.trim().is_empty())
            .map(|k| vec![ProviderKey::new(k)])
            .unwrap_or_default(),
        models: preset.default_models.clone(),
        auto_include: Vec::new(),
        catalog: preset.catalog.clone(),
        headers: BTreeMap::new(),
        user_agent: None,
        proxy: None,
        group_only: false,
        auth: preset.auth,
        chat_path: preset.chat_path.clone(),
        stream_only: preset.stream_only,
        hold_when_exhausted: false,
        media_models: preset.media_models.clone(),
        media_paths: preset.media_paths.clone(),
    }
}

/// A provider the user described by hand rather than by preset.
pub fn custom_provider(
    name: &str,
    base_url: Option<&str>,
    anthropic_base_url: Option<&str>,
    protocol: Protocol,
    key: Option<&str>,
    models: &[String],
) -> Provider {
    let id = slugify(name);
    Provider {
        id,
        name: name.to_string(),
        kind: "custom".to_string(),
        enabled: true,
        protocol,
        base_url: base_url.map(|s| s.trim_end_matches('/').to_string()),
        anthropic_base_url: anthropic_base_url.map(|s| s.trim_end_matches('/').to_string()),
        responses_base_url: None,
        gemini_base_url: None,
        keys: key
            .filter(|k| !k.trim().is_empty())
            .map(|k| vec![ProviderKey::new(k)])
            .unwrap_or_default(),
        models: models.to_vec(),
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

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "custom".to_string()
    } else {
        out
    }
}
