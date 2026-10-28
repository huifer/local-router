//! Trae CN Cloud-IDE sessions imported from Magpie and maintained locally.
//!
//! Trae is not a normal OpenAI-compatible upstream: it needs Cloud-IDE-JWT
//! headers and sends its own SSE event vocabulary. This module keeps the
//! imported accounts in a pool, refreshes JWTs with the account refresh token,
//! and exposes a small OpenAI-chat-shaped bridge to the gateway.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::protocol::ir::{
    BlockKind, ChatRequest, ContentBlock, Message, Role, StopReason, StreamEvent, ToolChoice, Usage,
};

pub const API_ENDPOINT: &str = "https://trae-api-cn.mchost.guru";
pub const AUTH_ENDPOINT: &str = "https://api.trae.cn";
pub const CHAT_PATH: &str = "/api/agent/v3/llm_utils_chat";
pub const REFRESH_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";
pub const CLIENT_ID: &str = "ono9krqynydwx5";
pub const APP_ID: &str = "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8";
pub const CLIENT_VERSION: &str = "0.1.69";
pub const CLIENT_VERSION_CODE: &str = "20260917";
pub const DEVICE_BRAND: &str = "ASUS TUF Gaming A15 FA507RM_FA507RM";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Account {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub account_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Auth {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub device_public_key: String,
    #[serde(default)]
    pub device_private_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionFile {
    #[serde(default)]
    pub account: Account,
    #[serde(default)]
    pub auth: Auth,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionPool {
    #[serde(default)]
    pub active: String,
    #[serde(default)]
    pub accounts: BTreeMap<String, SessionFile>,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub account: Account,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: Option<i64>,
    pub client_id: String,
    pub device_id: String,
    pub machine_id: String,
}

pub fn own_session_path() -> std::path::PathBuf {
    crate::config::config_dir().join("trae-session.json")
}

fn pool_path() -> std::path::PathBuf {
    own_session_path()
}

pub fn read_pool() -> anyhow::Result<SessionPool> {
    let path = pool_path();
    if !path.exists() {
        anyhow::bail!("no Trae CN session yet; import or sign in first")
    }
    let raw = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_pool(pool: &SessionPool) -> anyhow::Result<()> {
    let path = pool_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(pool)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
    }
    Ok(())
}

fn from_file(file: &SessionFile) -> Session {
    Session {
        account: file.account.clone(),
        access_token: file.auth.access_token.clone(),
        refresh_token: file.auth.refresh_token.clone(),
        expires_at: file.auth.expires_at,
        client_id: if file.auth.client_id.is_empty() {
            CLIENT_ID.into()
        } else {
            file.auth.client_id.clone()
        },
        device_id: file.auth.device_id.clone(),
        machine_id: file.auth.machine_id.clone(),
    }
}

pub fn session_of(name: &str) -> anyhow::Result<Session> {
    let pool = read_pool()?;
    let file = pool
        .accounts
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("there is no Trae CN account called {name}"))?;
    if file.auth.access_token.is_empty() {
        anyhow::bail!("Trae CN account {name} has no access token")
    }
    Ok(from_file(file))
}

pub fn persist_refreshed(name: &str, session: &Session) -> anyhow::Result<()> {
    let mut pool = read_pool()?;
    let file = pool
        .accounts
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("there is no Trae CN account called {name}"))?;
    file.auth.access_token = session.access_token.clone();
    file.auth.refresh_token = session.refresh_token.clone();
    file.auth.expires_at = session.expires_at;
    file.auth.client_id = session.client_id.clone();
    write_pool(&pool)
}

pub fn use_account(name: &str) -> anyhow::Result<()> {
    let mut pool = read_pool()?;
    if !pool.accounts.contains_key(name) {
        anyhow::bail!("there is no Trae CN account called {name}")
    }
    pool.active = name.to_string();
    write_pool(&pool)
}

pub fn remove_account(name: &str) -> anyhow::Result<bool> {
    let mut pool = read_pool()?;
    if pool.accounts.remove(name).is_none() {
        anyhow::bail!("there is no Trae CN account called {name}")
    }
    let was_active = pool.active == name;
    if was_active {
        pool.active = pool.accounts.keys().next().cloned().unwrap_or_default();
    }
    write_pool(&pool)?;
    Ok(was_active)
}

pub fn accounts() -> anyhow::Result<Vec<(String, SessionFile)>> {
    Ok(read_pool()?.accounts.into_iter().collect())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn needs_refresh(s: &Session) -> bool {
    s.expires_at
        .map(|at| at - now_ms() < 10 * 60 * 1000)
        .unwrap_or(false)
}

fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?)
}

pub async fn refresh(session: &Session) -> anyhow::Result<Session> {
    if session.refresh_token.trim().is_empty() {
        anyhow::bail!("Trae CN session has no refresh token")
    }
    let body = json!({"ClientID": if session.client_id.is_empty() { CLIENT_ID } else { &session.client_id }, "RefreshToken": session.refresh_token, "ClientSecret":"-", "UserID":""});
    let response = client()?
        .post(format!("{}{}", AUTH_ENDPOINT, REFRESH_PATH))
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let value: Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Trae CN refresh refused (HTTP {}): {}", status, value)
    }
    let result = value
        .get("Result")
        .or_else(|| value.get("result"))
        .unwrap_or(&value);
    let token = result
        .get("Token")
        .or_else(|| result.get("token"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if token.is_empty() {
        anyhow::bail!("Trae CN refresh returned no token")
    }
    let expires = result
        .get("TokenExpireAt")
        .or_else(|| result.get("tokenExpireAt"))
        .and_then(Value::as_i64);
    Ok(Session {
        access_token: token.into(),
        refresh_token: result
            .get("RefreshToken")
            .or_else(|| result.get("refreshToken"))
            .and_then(Value::as_str)
            .unwrap_or(&session.refresh_token)
            .into(),
        expires_at: expires.or(session.expires_at),
        ..session.clone()
    })
}

pub async fn session_rotating() -> anyhow::Result<(String, Session)> {
    let pool = read_pool()?;
    if pool.accounts.is_empty() {
        anyhow::bail!("no Trae CN accounts")
    }
    let names: Vec<_> = pool.accounts.keys().cloned().collect();
    static CURSOR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name =
        names[CURSOR.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % names.len()].clone();
    let current = session_of(&name)?;
    if !needs_refresh(&current) {
        return Ok((name, current));
    }
    match refresh(&current).await {
        Ok(fresh) => {
            let _ = persist_refreshed(&name, &fresh);
            Ok((name, fresh))
        }
        Err(e) => {
            tracing::warn!("Trae CN refresh failed: {e}");
            Ok((name, current))
        }
    }
}

fn headers(session: &Session, request_id: &str) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    let mut add = |k: &str, v: String| {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&v) {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v,
            );
        }
    };
    add(
        "authorization",
        format!("Cloud-IDE-JWT {}", session.access_token),
    );
    add("x-cloudide-token", session.access_token.clone());
    add("x-ide-token", session.access_token.clone());
    add("x-uid", session.account.uid.clone());
    add("x-app-id", APP_ID.into());
    add("x-device-id", session.device_id.clone());
    add("x-machine-id", session.machine_id.clone());
    add("x-request-id", request_id.into());
    add("x-app-version", CLIENT_VERSION.into());
    add("x-app-version-code", CLIENT_VERSION_CODE.into());
    add("x-ide-version", CLIENT_VERSION.into());
    add("x-ide-version-code", CLIENT_VERSION_CODE.into());
    add("x-ide-version-type", "stable".into());
    add("x-device-cpu", "AMD".into());
    add("x-device-brand", DEVICE_BRAND.into());
    add("x-device-type", "windows".into());
    add("x-os-version", "Windows 10".into());
    add("x-system-type", "Windows".into());
    add("content-type", "application/json".into());
    add("accept", "text/event-stream".into());
    h
}

fn message_text(m: &Message) -> String {
    m.content
        .iter()
        .map(ContentBlock::plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn body(req: &ChatRequest, model: &str) -> Value {
    let messages: Vec<Value> = req.messages.iter().map(|m| json!({"role": match m.role { Role::System => "system", Role::User => "user", Role::Assistant => "assistant", Role::Tool => "user" }, "content": [{"type":"text","text":message_text(m)}]})).collect();
    let mut out = json!({"messages": messages, "function":"chat_v3", "config_name":model, "model":model, "stream":true, "request_id":uuid::Uuid::new_v4().to_string(), "session_id":req.session_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string())});
    if let Some(max) = req.max_tokens {
        out["max_tokens"] = json!(max);
    }
    if let Some(temp) = req.temperature {
        out["temperature"] = json!(temp);
    }
    if !req.tools.is_empty() && !matches!(req.tool_choice, Some(ToolChoice::None)) {
        out["tools"] = json!(req.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.input_schema}})).collect::<Vec<_>>());
    }
    out
}

pub async fn chat(session: &Session, req: &ChatRequest) -> anyhow::Result<Vec<StreamEvent>> {
    let id = uuid::Uuid::new_v4().to_string();
    let response = client()?
        .post(format!("{}{}", API_ENDPOINT, CHAT_PATH))
        .headers(headers(session, &id))
        .json(&body(req, &req.model))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "Trae CN chat refused (HTTP {}): {}",
            response.status(),
            response.text().await.unwrap_or_default()
        )
    }
    parse_sse(response.bytes_stream()).await
}

async fn parse_sse(
    mut stream: impl futures_util::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
) -> anyhow::Result<Vec<StreamEvent>> {
    let mut buf = String::new();
    let mut out = vec![StreamEvent::MessageStart {
        id: uuid::Uuid::new_v4().to_string(),
        model: String::new(),
        usage: Usage::default(),
    }];
    let mut index = 0usize;
    let mut usage = Usage::default();
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk?));
        while let Some(pos) = buf.find("\n\n") {
            let frame = buf[..pos].to_string();
            buf = buf[pos + 2..].to_string();
            consume(&frame, &mut out, &mut index, &mut usage);
        }
    }
    if !buf.trim().is_empty() {
        consume(&buf, &mut out, &mut index, &mut usage);
    }
    out.push(StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::EndTurn),
        usage,
    });
    out.push(StreamEvent::MessageStop);
    Ok(out)
}

fn consume(frame: &str, out: &mut Vec<StreamEvent>, index: &mut usize, usage: &mut Usage) {
    let data = frame
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return;
    };
    let Ok(v) = serde_json::from_str::<Value>(&data) else {
        return;
    };
    let name = v
        .get("event")
        .or_else(|| v.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if name.eq_ignore_ascii_case("token_usage") {
        if let Some(u) = v.get("usage").unwrap_or(&v).as_object() {
            usage.input_tokens = u
                .get("prompt_tokens")
                .or_else(|| u.get("input_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            usage.output_tokens = u
                .get("completion_tokens")
                .or_else(|| u.get("output_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
        }
        return;
    }
    let text = v
        .get("response")
        .or_else(|| v.get("content"))
        .or_else(|| v.get("delta"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !text.is_empty() {
        if *index == 0 {
            out.push(StreamEvent::ContentBlockStart {
                index: *index,
                kind: BlockKind::Text,
            });
        }
        out.push(StreamEvent::TextDelta {
            index: *index,
            text: text.into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pool_path_is_stable() {
        assert!(own_session_path().ends_with("trae-session.json"));
    }
}
