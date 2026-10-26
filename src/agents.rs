//! Pointing an agent at the gateway.
//!
//! Each agent keeps its model in its own file, in its own format. local-router
//! rewrites the one line that matters and leaves everything else alone.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use serde_json::{json, Value};

/// Where the gateway is listening.
#[derive(Debug, Clone)]
pub struct Gateway {
    pub base_url: String,
    pub key: String,
}

impl Gateway {
    pub fn new(host: &str, port: u16, key: &str) -> Self {
        Gateway {
            base_url: format!("http://{host}:{port}"),
            key: key.to_string(),
        }
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Claude Code reads `~/.claude/settings.json`.
pub fn claude_settings_path() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join("settings.json");
    }
    home().join(".claude").join("settings.json")
}

/// Write the gateway into Claude Code's settings, keeping the rest.
pub fn set_claude(gateway: &Gateway, model: Option<&str>) -> anyhow::Result<PathBuf> {
    let path = claude_settings_path();
    let mut root = read_json(&path).unwrap_or_else(|| json!({}));
    let obj = root
        .as_object_mut()
        .context("the Claude settings file is not an object")?;
    let env = obj
        .entry("env")
        .or_insert_with(|| json!({}));
    let env = env.as_object_mut().context("the env block is not an object")?;
    env.insert(
        "ANTHROPIC_BASE_URL".into(),
        json!(gateway.base_url.clone()),
    );
    env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(gateway.key.clone()));
    if let Some(model) = model {
        env.insert("ANTHROPIC_MODEL".into(), json!(model));
    }
    write_json(&path, &root)?;
    Ok(path)
}

/// Codex keeps its model in `~/.codex/config.toml`.
pub fn codex_config_path() -> PathBuf {
    if let Ok(dir) = std::env::var("CODEX_HOME") {
        return PathBuf::from(dir).join("config.toml");
    }
    home().join(".codex").join("config.toml")
}

/// Write the gateway into Codex's config, keeping the rest.
pub fn set_codex(gateway: &Gateway, model: Option<&str>) -> anyhow::Result<PathBuf> {
    let path = codex_config_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = strip_keys(&existing, &["model", "model_provider"]);
    if let Some(model) = model {
        out.push_str(&format!("\nmodel = \"{model}\"\n"));
    }
    out.push_str("model_provider = \"local-router\"\n");
    out.push_str("\n[model_providers.local-router]\n");
    out.push_str("name = \"local-router\"\n");
    out.push_str(&format!("base_url = \"{}/v1\"\n", gateway.base_url));
    out.push_str("wire_api = \"responses\"\n");
    out.push_str("env_key = \"LOCAL_ROUTER_API_KEY\"\n");
    write_text(&path, &out)?;

    let auth_path = path.with_file_name("auth.json");
    let mut auth = read_json(&auth_path).unwrap_or_else(|| json!({}));
    if let Some(obj) = auth.as_object_mut() {
        obj.insert("OPENAI_API_KEY".into(), json!(gateway.key.clone()));
    }
    write_json(&auth_path, &auth)?;
    Ok(path)
}

/// Gemini CLI reads `~/.gemini/.env`.
pub fn set_gemini(gateway: &Gateway, model: Option<&str>) -> anyhow::Result<PathBuf> {
    let path = home().join(".gemini").join(".env");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|l| {
            !l.starts_with("GOOGLE_GEMINI_BASE_URL")
                && !l.starts_with("GEMINI_API_KEY")
                && !l.starts_with("GOOGLE_GEMINI_MODEL")
        })
        .map(|l| l.to_string())
        .collect();
    lines.push(format!("GOOGLE_GEMINI_BASE_URL={}", gateway.base_url));
    lines.push(format!("GEMINI_API_KEY={}", gateway.key));
    if let Some(model) = model {
        lines.push(format!("GOOGLE_GEMINI_MODEL={model}"));
    }
    write_text(&path, &(lines.join("\n") + "\n"))?;
    Ok(path)
}

/// OpenCode keeps its provider in `~/.config/opencode/opencode.json`.
pub fn set_opencode(gateway: &Gateway, model: Option<&str>) -> anyhow::Result<PathBuf> {
    let path = opencode_config_path();
    let mut root = read_json(&path).unwrap_or_else(|| json!({}));
    let obj = root
        .as_object_mut()
        .context("the OpenCode config is not an object")?;
    let provider = json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": "local-router",
        "options": {
            "baseURL": format!("{}/v1", gateway.base_url),
            "apiKey": gateway.key,
        },
        "models": {
            model.unwrap_or("local-router/model"): {"name": model.unwrap_or("local-router/model")}
        },
    });
    obj.insert("provider".into(), json!({"local-router": provider}));
    write_json(&path, &root)?;
    Ok(path)
}

pub fn opencode_config_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir)
            .join("opencode")
            .join("opencode.json");
    }
    home()
        .join(".config")
        .join("opencode")
        .join("opencode.json")
}

/// Remove some keys from a toml file, leaving the rest as it was.
fn strip_keys(text: &str, keys: &[&str]) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if skipping {
            if trimmed.starts_with('[') {
                skipping = false;
            } else {
                continue;
            }
        }
        if trimmed.starts_with('[') {
            skipping = false;
        } else if keys.iter().any(|k| {
            trimmed
                .split('=')
                .next()
                .map(|name| name.trim() == *k)
                .unwrap_or(false)
        }) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn read_json(path: &PathBuf) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_json(path: &PathBuf, value: &Value) -> anyhow::Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    write_text(path, &body)
}

fn write_text(path: &PathBuf, text: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// A summary of the agents local-router knows how to point at its gateway.
pub fn known_agents() -> Vec<(&'static str, &'static str)> {
    vec![
        ("claude", "Claude Code"),
        ("codex", "Codex"),
        ("gemini", "Gemini CLI"),
        ("opencode", "OpenCode"),
    ]
}

/// The environment an agent needs to talk to the gateway.
pub fn agent_env(gateway: &Gateway) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert(
        "OPENAI_BASE_URL".to_string(),
        format!("{}/v1", gateway.base_url),
    );
    env.insert("OPENAI_API_KEY".to_string(), gateway.key.clone());
    env.insert("ANTHROPIC_BASE_URL".to_string(), gateway.base_url.clone());
    env.insert("ANTHROPIC_API_KEY".to_string(), gateway.key.clone());
    env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), gateway.key.clone());
    env.insert(
        "GOOGLE_GEMINI_BASE_URL".to_string(),
        gateway.base_url.clone(),
    );
    env.insert("GEMINI_API_KEY".to_string(), gateway.key.clone());
    env
}
