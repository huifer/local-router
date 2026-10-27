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
    /// Glob patterns: a model the vendor newly lists is exposed on sight.
    /// A relay that renames its models often uses `["*"]`.
    #[serde(default)]
    pub auto_include: Vec<String>,
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
        auto_include: preset.auto_include.clone(),
        catalog: preset.catalog.clone(),
        headers: BTreeMap::new(),
        user_agent: None,
        forward_headers: Vec::new(),
        remove_headers: Vec::new(),
        header_order: Vec::new(),
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
        forward_headers: Vec::new(),
        remove_headers: Vec::new(),
        header_order: Vec::new(),
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
        // Unicode letters count too: a Chinese vendor name ("七牛云") must
        // not collapse into an empty slug, or every such provider would
        // be called "custom".
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_list_is_well_formed() {
        let presets = all();
        assert!(presets.len() >= 25, "{} presets", presets.len());
        // Ids are unique and slug-shaped; every vendor has a group and a URL.
        let mut ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "preset ids must be unique");
        for p in presets {
            assert!(!p.group.is_empty(), "{} has no group", p.id);
            let has_url = p.base_url.is_some()
                || p.anthropic_base_url.is_some()
                || p.gemini_base_url.is_some()
                || p.auth.is_some();
            assert!(
                has_url,
                "{} has neither a base url nor a session auth",
                p.id
            );
            // Template placeholders from the editor must never ride along:
            // they would expand to a model id no vendor knows.
            for model in &p.default_models {
                assert!(
                    !model.contains("{{") && !model.contains("SECRET"),
                    "{}/{} carries a template placeholder",
                    p.id,
                    model
                );
            }
        }
        // The groups the add sheet renders.
        let groups: Vec<String> = grouped().into_keys().collect();
        assert!(groups.contains(&"vendor".to_string()));
    }

    #[test]
    fn get_finds_known_ids_and_skips_unknown() {
        assert!(get("deepseek").is_some());
        assert!(get("workbuddy").is_some());
        assert!(get("qwenwork").is_some());
        assert!(get("no-such-preset").is_none());
    }

    #[test]
    fn session_presets_do_not_need_a_key() {
        for id in ["workbuddy", "qwenwork"] {
            let preset = get(id).unwrap();
            assert!(preset.auth.is_some(), "{id} must carry an auth kind");
            let provider = to_provider(preset, None);
            assert!(provider.uses_session());
            assert!(provider.keys.is_empty());
        }
    }

    #[test]
    fn to_provider_applies_the_preset_shape() {
        let preset = get("deepseek").unwrap();
        let provider = to_provider(preset, Some("sk-test"));
        assert_eq!(provider.id, "deepseek");
        assert_eq!(provider.name, preset.name);
        assert_eq!(provider.protocol, preset.protocol);
        assert_eq!(provider.base_url, preset.base_url);
        assert_eq!(provider.keys.len(), 1);
        assert_eq!(provider.keys[0].key, "sk-test");
        assert!(provider.enabled);
        assert_eq!(provider.models, preset.default_models);
        // A blank key means no key at all, not a blank secret.
        let empty = to_provider(preset, Some("   "));
        assert!(empty.keys.is_empty());
    }

    #[test]
    fn custom_providers_slug_their_name_and_fall_back() {
        let provider = custom_provider(
            "My Local Relay!",
            Some("http://127.0.0.1:9000/v1"),
            None,
            Protocol::OpenAiChat,
            Some("sk-x"),
            &["m1".to_string()],
        );
        assert_eq!(provider.id, "my-local-relay");
        assert_eq!(provider.models, vec!["m1"]);
        // A name with no slug characters lands on `custom`.
        let fallback = custom_provider("???", None, None, Protocol::Anthropic, None, &[]);
        assert_eq!(fallback.id, "custom");
        assert_eq!(fallback.protocol, Protocol::Anthropic);
    }
}
