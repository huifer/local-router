//! OAuth 2.0 (Authorization Code + PKCE) for upstreams that want a browser
//! login instead of an API key.
//!
//! The flow is the standard one, and it is deliberately generic: every
//! vendor spells it a little differently, so the endpoints live in the
//! provider's configuration rather than in the code.
//!
//! 1. The caller asks for a flow. We generate a PKCE verifier/challenge
//!    pair, remember the pending flow, and hand back the authorize URL.
//! 2. The user opens that URL, signs in with the vendor, and the vendor
//!    redirects back to us (`/oauth/callback`) with a code.
//! 3. We exchange the code for a token (a key, or a bearer token we can
//!    refresh) and store it as the provider's credential.
//!
//! The callback lands on the same origin as the console
//! (`http://127.0.0.1:3435/oauth/callback`), so the whole thing happens in
//! the browser the user is already looking at.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};

use anyhow::Context;

use crate::config::OAuthConfig;

/// How the code is exchanged for a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeStyle {
    /// The OAuth 2.0 standard: a form-encoded body with `grant_type`.
    Standard,
    /// OpenRouter's flavour: a JSON body with `code`/`code_verifier`, and
    /// the reply carries a `key` instead of an `access_token`.
    OpenRouter,
}

/// What a finished flow produced.
#[derive(Debug, Clone, Serialize)]
pub struct OAuthToken {
    /// The credential to send to the vendor.
    pub api_key: String,
    /// The refresh token, when the vendor gave one.
    pub refresh_token: Option<String>,
    /// When the credential expires (epoch millis), when the vendor said.
    pub expires_at: Option<i64>,
    /// The vendor's account id, when it said.
    pub user_id: Option<String>,
}

/// A flow in flight.
#[derive(Debug, Clone)]
struct Pending {
    provider_id: String,
    verifier: String,
    state: String,
    redirect_uri: String,
    config: OAuthConfig,
    started: std::time::Instant,
}

/// The pending flows of this process.
fn pending() -> &'static Arc<Mutex<BTreeMap<String, Pending>>> {
    static PENDING: OnceLock<Arc<Mutex<BTreeMap<String, Pending>>>> = OnceLock::new();
    PENDING.get_or_init(|| Arc::new(Mutex::new(BTreeMap::new())))
}

/// A PKCE verifier and its S256 challenge.
pub fn pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The public URL of this gateway (the console and the API share it).
fn public_base() -> String {
    let settings = crate::config::Store::new()
        .load()
        .map(|c| c.settings)
        .unwrap_or_default();
    format!("http://{}:{}", settings.gateway_addr, settings.gateway_port)
}

/// Start a flow: build the authorize URL and remember what it needs.
///
/// `redirect` is where the vendor sends the browser back to. When the
/// console drives the flow, that is this gateway's own `/oauth/callback`;
/// the CLI uses a loopback port of its own.
pub fn start_flow(
    provider_id: &str,
    config: &OAuthConfig,
    redirect_uri: &str,
) -> anyhow::Result<(String, String)> {
    let (verifier, challenge) = pkce_pair();
    let state = random_state();
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
        config.authorize_url,
        urlencode(&config.client_id),
        urlencode(redirect_uri),
        urlencode(&state),
        urlencode(&challenge),
    );
    if !config.scopes.is_empty() {
        url.push_str(&format!("&scope={}", urlencode(&config.scopes.join(" "))));
    }
    for (key, value) in &config.extra_params {
        url.push_str(&format!("&{}={}", urlencode(key), urlencode(value)));
    }
    let mut flows = pending().lock().unwrap();
    // Old flows expire: a user who abandoned one should not block the next.
    flows.retain(|_, flow| flow.started.elapsed() < std::time::Duration::from_secs(600));
    flows.insert(
        state.clone(),
        Pending {
            provider_id: provider_id.to_string(),
            verifier,
            state: state.clone(),
            redirect_uri: redirect_uri.to_string(),
            config: config.clone(),
            started: std::time::Instant::now(),
        },
    );
    Ok((url, state))
}

/// Finish a flow: swap the code for a credential.
pub async fn finish_flow(code: &str, state: &str) -> anyhow::Result<(String, OAuthToken)> {
    let flow = {
        let mut flows = pending().lock().unwrap();
        flows.remove(state)
    };
    let Some(flow) = flow else {
        anyhow::bail!("this login link has expired — start the sign-in again");
    };
    let token = exchange(&flow.config, code, &flow.verifier, &flow.redirect_uri).await?;
    Ok((flow.provider_id, token))
}

/// The provider and state of a flow that is still waiting.
pub fn flow_status(state: &str) -> Option<(String, String)> {
    let flows = pending().lock().unwrap();
    flows
        .get(state)
        .map(|f| (f.provider_id.clone(), f.state.clone()))
}

/// Exchange the code for a credential.
async fn exchange(
    config: &OAuthConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<OAuthToken> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let style = config.style();
    let (status, body) = match style {
        ExchangeStyle::OpenRouter => {
            let payload = serde_json::json!({
                "code": code,
                "code_challenge_method": "S256",
                "code_verifier": verifier,
            });
            let response = http
                .post(&config.token_url)
                .json(&payload)
                .send()
                .await
                .context("the sign-in could not reach the vendor")?;
            (
                response.status().as_u16(),
                response.text().await.unwrap_or_default(),
            )
        }
        ExchangeStyle::Standard => {
            let form = [
                ("grant_type", "authorization_code".to_string()),
                ("code", code.to_string()),
                ("client_id", config.client_id.clone()),
                ("redirect_uri", redirect_uri.to_string()),
                ("code_verifier", verifier.to_string()),
            ];
            let response = http
                .post(&config.token_url)
                .form(&form)
                .send()
                .await
                .context("the sign-in could not reach the vendor")?;
            (
                response.status().as_u16(),
                response.text().await.unwrap_or_default(),
            )
        }
    };
    if !(200..300).contains(&status) {
        anyhow::bail!(
            "the vendor refused the sign-in (HTTP {status}): {}",
            body.trim()
        );
    }
    let value: serde_json::Value =
        serde_json::from_str(&body).context("the vendor's reply was not json")?;
    // OpenRouter answers with `key`; the standard answers with
    // `access_token`.
    let api_key = value
        .get("key")
        .or_else(|| value.get("access_token"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("the vendor's reply carried no key: {}", body.trim()))?;
    let refresh_token = value
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let expires_at = value
        .get("expires_at")
        .or_else(|| value.get("expires_in"))
        .and_then(|v| v.as_i64())
        .map(|seconds| {
            if seconds > 1_000_000_000_000 {
                seconds // already epoch millis
            } else {
                crate::workbuddy::now_ms() + seconds * 1000
            }
        });
    let user_id = value
        .get("user_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(OAuthToken {
        api_key,
        refresh_token,
        expires_at,
        user_id,
    })
}

/// The redirect the console uses: this gateway's own callback path.
pub fn console_redirect() -> String {
    format!("{}/oauth/callback", public_base())
}

/// Percent-encode a value for a query string.
pub fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OAuthConfig {
        OAuthConfig {
            authorize_url: "https://vendor.example/auth".into(),
            token_url: "https://vendor.example/token".into(),
            client_id: "app-local-router".into(),
            scopes: vec!["openid".into(), "profile".into()],
            redirect_path: "/oauth/callback".into(),
            extra_params: BTreeMap::new(),
            style: None,
        }
    }

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let (verifier, challenge) = pkce_pair();
        assert!(verifier.len() >= 43);
        assert_ne!(verifier, challenge);
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
    }

    #[test]
    fn the_authorize_url_carries_everything_oauth_needs() {
        let (url, state) =
            start_flow("p", &config(), "http://127.0.0.1:3435/oauth/callback").unwrap();
        assert!(url.starts_with("https://vendor.example/auth?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("state={}", urlencode(&state))));
        assert!(url.contains("scope=openid%20profile"));
        // The pending flow is remembered, so the callback can be matched.
        let (provider, st) = flow_status(&state).unwrap();
        assert_eq!(provider, "p");
        assert_eq!(st, state);
    }

    #[test]
    fn extra_params_reach_the_authorize_url() {
        let mut cfg = config();
        cfg.extra_params
            .insert("audience".into(), "https://api.example".into());
        let (url, _) = start_flow("p", &cfg, "http://127.0.0.1:3435/oauth/callback").unwrap();
        assert!(url.contains("audience=https%3A%2F%2Fapi.example"));
    }

    #[tokio::test]
    async fn an_unknown_state_is_refused() {
        let err = finish_flow("code", "not-a-state").await.unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[test]
    fn urlencode_escapes_the_usual_suspects() {
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(urlencode("plain-._~"), "plain-._~");
    }
}
