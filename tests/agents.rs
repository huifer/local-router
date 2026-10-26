//! Pointing the coding agents at the gateway.
//!
//! The tests move the home directory around, so they take turns.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use local_router::agents::Gateway;
use serde_json::{json, Value};

/// One more unique number per call, so concurrent tests never share a file.
fn unique_n() -> u64 {
    static N: AtomicUsize = AtomicUsize::new(0);
    N.fetch_add(1, Ordering::SeqCst) as u64
}

/// The tests below move the environment around, so they take turns.
async fn env_guard() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

/// A temporary home directory.
struct Home {
    path: PathBuf,
}

impl Home {
    fn new() -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "local-router-agents-{}-{}",
            std::process::id(),
            unique_n()
        ));
        std::fs::create_dir_all(&path).expect("home dir");
        Home { path }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn gateway() -> Gateway {
    Gateway::new("127.0.0.1", 13425, "caller-key")
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_keeps_the_rest_of_its_settings() {
    let _turn = env_guard().await;
    let home = Home::new();
    let claude_dir = home.path.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        json!({"permissions": {"allow": ["Bash"]}}).to_string(),
    )
    .unwrap();
    std::env::set_var("HOME", &home.path);
    std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir);

    let path = local_router::agents::set_claude(&gateway(), Some("mock/model")).unwrap();
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).expect("valid json");
    // The rest of the file is untouched.
    assert_eq!(written["permissions"]["allow"][0], "Bash");
    // The gateway is now the one Claude Code talks to.
    assert_eq!(
        written["env"]["ANTHROPIC_BASE_URL"],
        "http://127.0.0.1:13425"
    );
    assert_eq!(written["env"]["ANTHROPIC_AUTH_TOKEN"], "caller-key");
    assert_eq!(written["env"]["ANTHROPIC_MODEL"], "mock/model");

    std::env::remove_var("HOME");
    std::env::remove_var("CLAUDE_CONFIG_DIR");
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_gets_a_provider_of_its_own() {
    let _turn = env_guard().await;
    let home = Home::new();
    let codex_dir = home.path.join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(codex_dir.join("config.toml"), "model = \"old\"\n").unwrap();
    std::env::set_var("HOME", &home.path);
    std::env::set_var("CODEX_HOME", &codex_dir);

    let path = local_router::agents::set_codex(&gateway(), Some("mock/model")).unwrap();
    let toml = std::fs::read_to_string(&path).unwrap();
    assert!(toml.contains("model = \"mock/model\""), "{toml}");
    assert!(toml.contains("[model_providers.local-router]"), "{toml}");
    assert!(
        toml.contains("base_url = \"http://127.0.0.1:13425/v1\""),
        "{toml}"
    );
    // The old model line is replaced, not kept next to the new one.
    assert_eq!(toml.matches("model = ").count(), 1, "{toml}");

    let auth: Value =
        serde_json::from_str(&std::fs::read_to_string(codex_dir.join("auth.json")).unwrap())
            .expect("valid json");
    assert_eq!(auth["OPENAI_API_KEY"], "caller-key");

    std::env::remove_var("HOME");
    std::env::remove_var("CODEX_HOME");
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_gets_its_environment() {
    let _turn = env_guard().await;
    let home = Home::new();
    let gemini_dir = home.path.join(".gemini");
    std::fs::create_dir_all(&gemini_dir).unwrap();
    std::fs::write(gemini_dir.join(".env"), "GEMINI_API_KEY=old\nOTHER=1\n").unwrap();
    std::env::set_var("HOME", &home.path);

    let path = local_router::agents::set_gemini(&gateway(), Some("mock/model")).unwrap();
    let env = std::fs::read_to_string(&path).unwrap();
    assert!(
        env.contains("GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:13425"),
        "{env}"
    );
    assert!(env.contains("GEMINI_API_KEY=caller-key"), "{env}");
    assert!(env.contains("GOOGLE_GEMINI_MODEL=mock/model"), "{env}");
    // The old key is replaced, not duplicated.
    assert_eq!(env.matches("GEMINI_API_KEY=").count(), 1, "{env}");
    // Other lines survive.
    assert!(env.contains("OTHER=1"), "{env}");

    std::env::remove_var("HOME");
}

#[tokio::test(flavor = "multi_thread")]
async fn opencode_keeps_the_other_providers() {
    let _turn = env_guard().await;
    let home = Home::new();
    let config_dir = home.path.join(".config/opencode");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("opencode.json"),
        json!({
            "provider": {
                "someone-else": {"npm": "@ai-sdk/other", "options": {"apiKey": "keep-me"}}
            },
            "theme": "dark"
        })
        .to_string(),
    )
    .unwrap();
    std::env::set_var("HOME", &home.path);
    std::env::set_var("XDG_CONFIG_HOME", home.path.join(".config"));

    let path = local_router::agents::set_opencode(&gateway(), Some("mock/model")).unwrap();
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).expect("valid json");
    // The other provider is still there.
    assert_eq!(
        written["provider"]["someone-else"]["options"]["apiKey"],
        "keep-me"
    );
    // And ours was added.
    assert_eq!(
        written["provider"]["local-router"]["options"]["baseURL"],
        "http://127.0.0.1:13425/v1"
    );
    // The rest of the file is untouched.
    assert_eq!(written["theme"], "dark");

    std::env::remove_var("HOME");
    std::env::remove_var("XDG_CONFIG_HOME");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gateway_on_every_interface_is_written_as_the_loopback_address() {
    let gateway = Gateway::new("0.0.0.0", 13425, "caller-key");
    assert_eq!(gateway.base_url, "http://127.0.0.1:13425");
}
