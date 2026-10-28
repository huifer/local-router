//! The gateway: the HTTP surface every agent talks to.

pub mod client;
pub mod route;
pub mod usage;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::config::{AppState, Provider};
use crate::gateway::usage::UsageRecord;
use crate::protocol::ir::*;
use crate::protocol::{anthropic, gemini, openai_chat, openai_responses, sse};
use crate::store;

use client::{Target, UpstreamReply};

/// Where the request says its working directory is, if anywhere.
///
/// Harnesses differ: some send a dedicated header, some a body field, some
/// only embed the directory inside Anthropic's `metadata.user_id`. All three
/// are accepted; the first that says anything wins.
fn request_project_dir(body: &Value, headers: &HeaderMap) -> Option<String> {
    // A header is the most explicit: `x-project: /path` or `x-cwd: /path`.
    for name in ["x-project", "x-project-dir", "x-cwd"] {
        if let Some(v) = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return Some(v);
        }
    }
    // A body field the agent chose to add.
    for pointer in ["/metadata/cwd", "/metadata/project_dir", "/cwd"] {
        if let Some(v) = body
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return Some(v);
        }
    }
    None
}

/// Claude Code packs its working directory into the session id it sends as
/// `metadata.user_id`: `user_<hash>__account__<uuid>__cwd_<path>`. Pull the
/// path out so those requests group into projects too.
fn extract_cwd_from_session(session: &str) -> Option<String> {
    let marker = "__cwd_";
    let pos = session.find(marker)?;
    let dir = session[pos + marker.len()..].trim();
    (!dir.is_empty() && dir.starts_with('/') || dir.starts_with('~')).then(|| dir.to_string())
}

/// Turn a caller's User-Agent into the coding agent it is, when it is one.
///
/// Agents that route through the gateway send their own User-Agent (Claude
/// Code, Codex, Gemini CLI, Pi, OpenCode…). magpie groups usage by agent; this
/// names the agent so the Usage page can do the same. A generic HTTP client
/// or browser maps to `None` and is shown as "unknown".
fn agent_from_user_agent(ua: &str) -> Option<String> {
    let ua = ua.to_ascii_lowercase();
    let known = [
        "claude",
        "codex",
        "gemini-cli",
        "gemini_code",
        "opencode",
        "pi/",
        "pi ",
        "cursor",
        "aider",
        "continue",
        "cline",
        "goose",
        "copilot",
        "crush",
        "openrouter",
        "zed",
        "windsurf",
        "trae",
        "qwen",
        "zai",
    ];
    for token in known {
        if ua.contains(token) {
            // The agent's own short name, as the Usage page labels it.
            let name = match token {
                "claude" => "claude-code",
                "codex" => "codex",
                "gemini-cli" | "gemini_code" => "gemini-cli",
                "pi/" | "pi " => "pi",
                "copilot" => "copilot",
                "opencode" => "opencode",
                "cursor" => "cursor",
                "aider" => "aider",
                "continue" => "continue",
                "cline" => "cline",
                "goose" => "goose",
                "crush" => "crush",
                "openrouter" => "openrouter",
                "zed" => "zed",
                "windsurf" => "windsurf",
                "trae" => "trae",
                "qwen" => "qwen",
                "zai" => "zai",
                _ => token,
            };
            return Some(name.to_string());
        }
    }
    None
}

#[derive(Clone)]
struct MediaTask {
    target: Target,
    provider: Provider,
    /// The upstream's answer when the task was created. A vendor that
    /// finishes the video straight away has no task endpoint of its own, so
    /// the gateway keeps the answer and serves it.
    body: Value,
}
use route::{RoutePlan, Router};

/// Everything a gateway handler needs.
#[derive(Clone)]
pub struct Gateway {
    pub state: AppState,
    pub router: Arc<Router>,
    pub clients: Arc<Mutex<HashMap<String, reqwest::Client>>>,
    /// Video tasks are asynchronous upstream resources. Keep the route to
    /// the provider long enough for status/content/delete calls.
    media_tasks: Arc<Mutex<HashMap<String, MediaTask>>>,
    /// The SQLite store, when it is on.
    pub sqlite: Option<store::Store>,
}

impl Gateway {
    pub fn new(state: AppState) -> Self {
        Gateway {
            state,
            router: Arc::new(Router::new()),
            clients: Arc::new(Mutex::new(HashMap::new())),
            media_tasks: Arc::new(Mutex::new(HashMap::new())),
            sqlite: store::Store::shared(),
        }
    }

    async fn client_for(
        &self,
        provider_id: &str,
        provider: &Provider,
    ) -> anyhow::Result<reqwest::Client> {
        let mut cache = self.clients.lock().await;
        if let Some(c) = cache.get(provider_id) {
            return Ok(c.clone());
        }
        let c = client::build_client(provider)?;
        cache.insert(provider_id.to_string(), c.clone());
        Ok(c)
    }
}

/// Build the gateway's routes.
pub fn router(gateway: Gateway) -> axum::Router {
    let router = axum::Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/models/{provider}/{model}", get(get_model))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/images/generations", post(image_generations))
        .route("/v1/images/edits", post(image_edits))
        .route("/v1/videos", post(video_create))
        .route("/v1/videos/{id}", get(video_status).delete(video_delete))
        .route("/v1/videos/{id}/content", get(video_content))
        .route("/v1/responses", post(responses))
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens))
        .route("/v1beta/models/{*model}", post(gemini_route))
        .route("/v1/local-router/route", get(route_info))
        .route("/v1/local-router/quotas", get(quotas))
        .route("/v1/local-router/usage", get(usage_summary))
        .route("/v1/local-router/usage/requests", get(usage_requests))
        .route("/v1/local-router/status", get(status))
        .with_state(gateway.clone());
    // A browser on another site may only talk to the gateway when the
    // user says so.
    let origins = match gateway.state.config.try_read() {
        Ok(config) => config.settings.cors_origins.clone(),
        Err(_) => {
            tracing::warn!("the CORS setting could not be read; no origin is allowed");
            Vec::new()
        }
    };
    if !origins.is_empty() {
        let allow_any = origins.iter().any(|o| o == "*");
        let layer = if allow_any {
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::AllowMethods::any())
                .allow_headers(tower_http::cors::Any)
        } else {
            let allowed: Vec<axum::http::HeaderValue> = origins
                .iter()
                .filter_map(|origin| match origin.parse() {
                    Ok(value) => Some(value),
                    Err(_) => {
                        tracing::warn!("{origin} is not a valid origin; it is not allowed");
                        None
                    }
                })
                .collect();
            tower_http::cors::CorsLayer::new()
                .allow_origin(allowed)
                .allow_methods(tower_http::cors::AllowMethods::any())
                .allow_headers(tower_http::cors::Any)
        };
        router.layer(layer)
    } else {
        router
    }
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok", "version": client::VERSION}))
}

async fn status(State(g): State<Gateway>) -> Json<Value> {
    let config = g.state.config.read().await;
    Json(json!({
        "version": client::VERSION,
        "providers": config.providers.len(),
        "groups": config.groups.len(),
        "models": config.model_ids().len(),
    }))
}

/// The models the gateway serves, in the OpenAI list shape.
async fn list_models(State(g): State<Gateway>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let config = g.state.config.read().await;
    let mut data = Vec::new();
    let created = chrono::Utc::now().timestamp();
    for p in &config.providers {
        if !p.enabled {
            continue;
        }
        for m in &p.models {
            data.push(json!({
                "id": format!("{}/{}", p.id, m),
                "object": "model",
                "created": created,
                "owned_by": p.id,
                "display_name": format!("{} · {}", p.name, m),
                "modalities": {"input": ["text"], "output": ["text"]},
                "native_endpoints": ["/v1/chat/completions"],
            }));
        }
        // The media models are listed too, so a client can find out which
        // model draws or films.
        for m in &p.media_models {
            data.push(json!({
                "id": format!("{}/{}", p.id, m),
                "object": "model",
                "created": created,
                "owned_by": p.id,
                "display_name": format!("{} · {}", p.name, m),
                "modalities": {"input": ["text"], "output": ["image", "video"]},
                "native_endpoints": [
                    "/v1/images/generations",
                    "/v1/images/edits",
                    "/v1/videos",
                ],
            }));
        }
    }
    for group in &config.groups {
        if !group.listed {
            continue;
        }
        data.push(json!({
            "id": format!("group/{}", group.id),
            "object": "model",
            "created": created,
            "owned_by": "local-router",
            "display_name": format!("{} · group", group.name),
        }));
    }
    Json(json!({"object": "list", "data": data})).into_response()
}

async fn get_model(
    State(g): State<Gateway>,
    Path((provider, model)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let config = g.state.config.read().await;
    let id = format!("{provider}/{model}");
    if !config.model_ids().contains(&id) {
        return error_response(&ApiError::NotFound(format!("there is no model {id}")));
    }
    Json(json!({
        "id": id,
        "object": "model",
        "created": chrono::Utc::now().timestamp(),
        "owned_by": provider,
    }))
    .into_response()
}

/// What the routing view shows.
async fn route_info(State(g): State<Gateway>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let config = g.state.config.read().await;
    // The session a caller named, if it named one.
    let session = headers
        .get("x-session-id")
        .or_else(|| headers.get("x-magpie-session"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    drop(config);
    let trace = match &session {
        Some(s) => g.router.trace_of(s).await,
        None => None,
    };
    let groups: Vec<Value> = {
        let config = g.state.config.read().await;
        config
            .groups
            .iter()
            .map(|group| {
                json!({
                    "id": group.id,
                    "name": group.name,
                    "routing": group.routing.as_str(),
                    "stays": group.stays.as_str(),
                    "members": group.models,
                    "listed": group.listed,
                })
            })
            .collect()
    };
    let mut out = json!({"groups": groups});
    if let Some(trace) = trace {
        out["session"] = json!(trace);
    }
    Json(out).into_response()
}

/// The quota windows of every provider that reports one.
async fn quotas(State(g): State<Gateway>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let config = g.state.config.read().await;
    let session_based = config
        .providers
        .iter()
        .filter(|p| p.enabled && p.uses_session())
        .map(|p| p.id.clone())
        .collect::<Vec<_>>();
    drop(config);

    let mut data = Vec::new();
    if session_based.iter().any(|id| id == "workbuddy") {
        match crate::workbuddy::quota().await {
            Ok(quota) => data.push(serde_json::to_value(quota).unwrap_or(Value::Null)),
            Err(err) => {
                return error_response(&ApiError::Upstream(err.to_string()));
            }
        }
    }
    if session_based.iter().any(|id| id == "qwenwork") {
        match crate::qwen::login::session().await {
            Ok(session) => match crate::qwen::client::account_quota(&session).await {
                Ok(q) => data.push(serde_json::json!({
                    "provider": "qwenwork",
                    "name": "QwenWork（千问办公）",
                    "kind": "subscription",
                    "plan": q.plan,
                    "user": session.account.nickname,
                    "windows": [{
                        "name": "Credits",
                        "used": Value::Null,
                        "remaining": q.pools.iter().map(|p| p.balance).sum::<f64>(),
                        "resets_at": q.resets_at,
                        "display": q.display,
                        "unit": "credits",
                        "unlimited": false,
                        "holds_why": if q.empty { Some("balance is zero".to_string()) } else { None },
                    }],
                    "pools": q.pools,
                    "read_at": chrono::Utc::now().to_rfc3339(),
                })),
                Err(err) => {
                    return error_response(&ApiError::Upstream(err.to_string()));
                }
            },
            Err(err) => {
                return error_response(&ApiError::Upstream(err.to_string()));
            }
        }
    }
    Json(json!({"data": data})).into_response()
}

/// The period a usage query covers.
fn usage_period(query: &HashMap<String, String>) -> (&'static str, i64) {
    match query.get("period").map(|s| s.as_str()) {
        Some("today") => ("today", 1),
        Some("24h") => ("24h", 1),
        Some("30d") => ("30d", 30),
        Some("all") => ("all", 36500),
        _ => ("7d", 7),
    }
}

/// The account's usage over a period.
async fn usage_summary(
    State(g): State<Gateway>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let (period, days) = usage_period(&query);
    let records = usage::read_all(&g.state.usage_path());
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let recent: Vec<_> = records
        .into_iter()
        .filter(|r| {
            chrono::DateTime::parse_from_rfc3339(&r.time)
                .map(|t| t.with_timezone(&chrono::Utc) >= cutoff)
                .unwrap_or(true)
        })
        .collect();
    let totals = usage::totals(&recent);
    let by_model = usage::group_by(&recent, |r| r.model.clone());
    let by_provider = usage::group_by(&recent, |r| r.provider.clone());
    let by_day = usage::by_day(&recent);
    // The per-platform money view: what each provider cost, this period.
    let per_provider: Vec<Value> = by_provider
        .iter()
        .map(|(provider, totals)| {
            json!({
                "provider": provider,
                "calls": totals.requests,
                "failed": totals.failed,
                "input": totals.input_tokens,
                "output": totals.output_tokens,
                "cost_usd": totals.cost_usd,
                "credits": totals.credits,
                "unpriced": totals.unpriced,
            })
        })
        .collect();
    Json(json!({
        "period": period,
        "since": cutoff.to_rfc3339(),
        "calls": totals.requests,
        "errors": totals.failed,
        "input": totals.input_tokens,
        "output": totals.output_tokens,
        "cache_read": totals.cache_read_tokens,
        "cache_write": totals.cache_write_tokens,
        "reasoning": totals.reasoning_tokens,
        "cost_usd": totals.cost_usd,
        "credits": totals.credits,
        "unpriced": totals.unpriced,
        "per_provider": per_provider,
        "by_model": by_model,
        "by_provider": by_provider,
        "by_day": by_day,
    }))
    .into_response()
}

/// The requests behind the summary, newest last.
async fn usage_requests(
    State(g): State<Gateway>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let (period, days) = usage_period(&query);
    let records = usage::read_all(&g.state.usage_path());
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let recent: Vec<_> = records
        .into_iter()
        .filter(|r| {
            chrono::DateTime::parse_from_rfc3339(&r.time)
                .map(|t| t.with_timezone(&chrono::Utc) >= cutoff)
                .unwrap_or(true)
        })
        .collect();
    let totals = usage::totals(&recent);
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100)
        .min(1000);
    let rows: Vec<&UsageRecord> = recent.iter().rev().take(limit).collect();
    Json(json!({
        "period": period,
        "rows": rows,
        "total": recent.len(),
        "calls": totals.requests,
        "errors": totals.failed,
        "input": totals.input_tokens,
        "output": totals.output_tokens,
        "cost_usd": totals.cost_usd,
        "credits": totals.credits,
        "unpriced": totals.unpriced,
    }))
    .into_response()
}

// ------------------------------------------------------------------ auth --

/// The keys a caller may use. Shared with the web console's remote guard.
pub(crate) fn allowed_keys(state: &AppState) -> Vec<String> {
    let Ok(config) = state.config.try_read() else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    // The environment key is process-local and never written back to disk,
    // so it is read at call time rather than folded into the file.
    if let Ok(key) = std::env::var("LOCAL_ROUTER_DEFAULT_KEY") {
        if !key.trim().is_empty() {
            keys.push(key.trim().to_string());
        }
    }
    if !config.settings.default_caller_key.is_empty() {
        keys.push(config.settings.default_caller_key.clone());
    }
    for k in config.caller_keys.iter().filter(|k| k.enabled) {
        keys.push(k.key.clone());
    }
    keys
}

/// The key a caller presented, over any of the header styles agents use.
pub(crate) fn presented_key(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        let raw = v.strip_prefix("Bearer ").unwrap_or(v).trim();
        if !raw.is_empty() {
            return Some(raw.to_string());
        }
    }
    for name in ["x-api-key", "x-goog-api-key", "api-key"] {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            if !v.trim().is_empty() {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// The auth check every endpoint goes through.
///
/// It reloads the configuration first when the file changed, so a key or a
/// provider added by hand (or by the CLI) takes effect on the very next
/// request — on any endpoint, not just the chat ones.
async fn authorize(g: &Gateway, headers: &HeaderMap) -> Result<(), ApiError> {
    g.state.sync().await;
    check_auth(g, headers)
}

fn check_auth(g: &Gateway, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(presented) = presented_key(headers) else {
        return Err(ApiError::Unauthorized(
            "this request has no key; send Authorization: Bearer <key>".into(),
        ));
    };
    let keys = allowed_keys(&g.state);
    if keys.is_empty() || keys.contains(&presented) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized("that key is not accepted".into()))
    }
}

// -------------------------------------------------------------- handlers --

async fn chat_completions(
    State(g): State<Gateway>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    handle(g, Protocol::OpenAiChat, headers, body).await
}

async fn responses(
    State(g): State<Gateway>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    handle(g, Protocol::OpenAiResponses, headers, body).await
}

async fn messages(
    State(g): State<Gateway>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    handle(g, Protocol::Anthropic, headers, body).await
}

/// Image generation uses the provider's native OpenAI-compatible endpoint.
/// It deliberately bypasses the chat IR: image payloads contain provider
/// specific fields and should pass through unchanged.
async fn image_generations(
    State(g): State<Gateway>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let requested = body.get("model").and_then(Value::as_str).unwrap_or("");
    let target = match media_target(&g, requested).await {
        Ok(target) => target,
        Err(e) => return error_response(&e),
    };
    body["model"] = Value::String(target.model.clone());
    match send_media_json(&g, &target, &body, MediaKind::ImageGenerations).await {
        Ok(response) => response,
        Err(e) => error_response(&e),
    }
}

/// Image editing accepts OpenAI's multipart form (model, prompt, image,
/// mask and optional generation fields) and forwards it as multipart.
async fn image_edits(
    State(g): State<Gateway>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let mut fields: Vec<(String, Option<String>, Option<String>, bytes::Bytes)> = Vec::new();
    let mut model = String::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("field").to_string();
        let file_name = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(str::to_string);
        let bytes = match field.bytes().await {
            Ok(bytes) => bytes,
            Err(err) => return error_response(&ApiError::BadRequest(err.to_string())),
        };
        if name == "model" {
            model = String::from_utf8_lossy(&bytes).to_string();
        }
        fields.push((name, file_name, content_type, bytes));
    }
    let target = match media_target(&g, &model).await {
        Ok(target) => target,
        Err(e) => return error_response(&e),
    };
    let mut form = reqwest::multipart::Form::new();
    for (name, file_name, content_type, bytes) in fields {
        let mut part = if let Some(content_type) = content_type {
            reqwest::multipart::Part::bytes(bytes.to_vec())
                .mime_str(&content_type)
                .unwrap_or_else(|_| reqwest::multipart::Part::bytes(bytes.to_vec()))
        } else {
            reqwest::multipart::Part::bytes(bytes.to_vec())
        };
        if let Some(file_name) = file_name {
            part = part.file_name(file_name);
        }
        form = form.part(name, part);
    }
    match send_media_multipart(&g, &target, form, MediaKind::ImageEdits).await {
        Ok(response) => response,
        Err(e) => error_response(&e),
    }
}

/// Create an asynchronous video generation task.
async fn video_create(
    State(g): State<Gateway>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let requested = body.get("model").and_then(Value::as_str).unwrap_or("");
    let target = match media_target(&g, requested).await {
        Ok(target) => target,
        Err(e) => return error_response(&e),
    };
    body["model"] = Value::String(target.model.clone());
    let provider = match g
        .state
        .config
        .read()
        .await
        .provider(&target.provider_id)
        .cloned()
    {
        Some(provider) => provider,
        None => return error_response(&ApiError::NotFound("the provider disappeared".into())),
    };
    let (response, upstream_body) =
        match send_media_json_raw(&g, &target, &body, MediaKind::VideoCreate).await {
            Ok(value) => value,
            Err(e) => return error_response(&e),
        };
    let upstream: Value = serde_json::from_slice(&upstream_body).unwrap_or(Value::Null);
    if let Some(id) = upstream.get("id").and_then(Value::as_str) {
        g.media_tasks.lock().await.insert(
            id.to_string(),
            MediaTask {
                target,
                provider,
                body: upstream,
            },
        );
    }
    response
}

/// Retrieve a video task's current state.
async fn video_status(
    State(g): State<Gateway>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    media_task_request(&g, id, MediaKind::VideoStatus, "GET", headers).await
}

/// Cancel/delete a video task and forget its provider route.
async fn video_delete(
    State(g): State<Gateway>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let response =
        media_task_request(&g, id.clone(), MediaKind::VideoDelete, "DELETE", headers).await;
    g.media_tasks.lock().await.remove(&id);
    response
}

/// Download the completed video bytes.
async fn video_content(
    State(g): State<Gateway>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    media_task_request(&g, id, MediaKind::VideoContent, "GET", headers).await
}

/// Find the first configured target for a media request.
///
/// A media model may live in the provider's media list rather than its
/// chat list, so both are consulted.
async fn media_target(g: &Gateway, requested: &str) -> Result<Target, ApiError> {
    if requested.trim().is_empty() {
        return Err(ApiError::BadRequest("the request has no model".into()));
    }
    let (provider_id, model) = match requested.split_once('/') {
        Some((provider_id, model)) => (provider_id.to_string(), model.to_string()),
        None => (String::new(), requested.to_string()),
    };
    let config = g.state.config.read().await;
    let provider = if provider_id.is_empty() {
        config
            .providers
            .iter()
            .find(|p| p.enabled && p.media_models.contains(&model))
            .or_else(|| {
                config
                    .providers
                    .iter()
                    .find(|p| p.enabled && p.models.contains(&model))
            })
    } else {
        config
            .providers
            .iter()
            .find(|p| p.id == provider_id && p.enabled)
    };
    let Some(provider) = provider.cloned() else {
        return Err(ApiError::NotFound(format!(
            "there is no provider called {provider_id}"
        )));
    };
    if !provider.media_models.contains(&model) && !provider.models.contains(&model) {
        return Err(ApiError::NotFound(format!(
            "the provider {} does not serve {model}",
            provider.id
        )));
    }
    let secret = provider
        .enabled_keys()
        .next()
        .map(|k| k.key.clone())
        .unwrap_or_default();
    let key_id = provider
        .enabled_keys()
        .next()
        .map(|k| k.id.clone())
        .unwrap_or_else(|| "session".to_string());
    Ok(Target {
        provider_id: provider.id.clone(),
        provider_name: provider.name.clone(),
        key_id,
        key_label: None,
        secret,
        model,
        protocol: provider.protocol,
        asked: requested.to_string(),
    })
}

/// Make the base URL for an auxiliary media endpoint.
///
/// A provider may point its media endpoints somewhere else than the OpenAI
/// `/v1` family — WorkBuddy keeps them under `/v2` next to its chat
/// endpoint — so the path is configurable.
fn media_url(
    provider: &Provider,
    target: &Target,
    kind: MediaKind,
    id: Option<&str>,
) -> Result<String, ApiError> {
    let base = provider
        .base_url_for(target.protocol)
        .ok_or_else(|| ApiError::BadRequest("the provider has no url".into()))?;
    let configured = provider.media_paths.as_ref().and_then(|paths| match kind {
        MediaKind::ImageGenerations => paths.image_generations.clone(),
        MediaKind::ImageEdits => paths.image_edits.clone(),
        MediaKind::VideoCreate => paths.video_create.clone(),
        MediaKind::VideoStatus => paths.video_status.clone(),
        MediaKind::VideoContent => paths.video_content.clone(),
        MediaKind::VideoDelete => paths.video_delete.clone(),
    });
    let path = match configured {
        Some(path) => path,
        None => {
            let suffix = match kind {
                MediaKind::ImageGenerations => "images/generations",
                MediaKind::ImageEdits => "images/edits",
                MediaKind::VideoCreate => "videos",
                MediaKind::VideoStatus => "videos/{id}",
                MediaKind::VideoContent => "videos/{id}/content",
                MediaKind::VideoDelete => "videos/{id}",
            };
            let suffix = match id {
                Some(id) => suffix.replace("{id}", id),
                None => suffix.to_string(),
            };
            client::auxiliary_path(&base, &suffix)
        }
    };
    Ok(format!("{}{}", base.trim_end_matches('/'), path))
}

/// Which media endpoint is being addressed.
#[derive(Clone, Copy)]
enum MediaKind {
    ImageGenerations,
    ImageEdits,
    VideoCreate,
    VideoStatus,
    VideoContent,
    VideoDelete,
}

/// Forward a JSON media request and preserve the upstream body/status.
async fn send_media_json(
    g: &Gateway,
    target: &Target,
    body: &Value,
    kind: MediaKind,
) -> Result<Response, ApiError> {
    let (response, _) = send_media_json_raw(g, target, body, kind).await?;
    Ok(response)
}

async fn send_media_json_raw(
    g: &Gateway,
    target: &Target,
    body: &Value,
    kind: MediaKind,
) -> Result<(Response, bytes::Bytes), ApiError> {
    let provider = g
        .state
        .config
        .read()
        .await
        .provider(&target.provider_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("the provider disappeared".into()))?;
    let headers = target_headers(&provider, target, None, false, None).await?;
    let url = media_url(&provider, target, kind, None)?;
    tracing::debug!("media request to {url}");
    let http = g
        .client_for(&target.provider_id, &provider)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut request = http.post(url).json(body);
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?;
    media_response_raw(response).await
}

/// Forward a multipart image edit.
async fn send_media_multipart(
    g: &Gateway,
    target: &Target,
    form: reqwest::multipart::Form,
    kind: MediaKind,
) -> Result<Response, ApiError> {
    let provider = g
        .state
        .config
        .read()
        .await
        .provider(&target.provider_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("the provider disappeared".into()))?;
    let headers = target_headers(&provider, target, None, false, None).await?;
    let url = media_url(&provider, target, kind, None)?;
    let http = g
        .client_for(&target.provider_id, &provider)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut request = http.post(url).multipart(form);
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?;
    media_response(response).await
}

/// Preserve media status, content type and bytes. Errors are translated to
/// the gateway's normal JSON error shape by the caller.
async fn media_response(response: reqwest::Response) -> Result<Response, ApiError> {
    let (response, _) = media_response_raw(response).await?;
    Ok(response)
}

async fn media_response_raw(
    response: reqwest::Response,
) -> Result<(Response, bytes::Bytes), ApiError> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let body = response
        .bytes()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?;
    if !status.is_success() {
        let value: Value = serde_json::from_slice(&body)
            .unwrap_or_else(|_| json!({"error": {"message": String::from_utf8_lossy(&body)}}));
        let message = error_message(&value);
        return Err(match status.as_u16() {
            400..=499 => ApiError::BadRequest(message),
            _ => ApiError::Upstream(message),
        });
    }
    let saved = body.clone();
    let response = Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(Body::from(body))
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((response, saved))
}

/// Forward a status/delete/content request for a task created by this
/// process. The task mapping is intentionally in-memory; if the process
/// restarts, clients should submit the task again or use the provider's
/// native API.
async fn media_task_request(
    g: &Gateway,
    id: String,
    kind: MediaKind,
    method: &str,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = authorize(g, &headers).await {
        return error_response(&e);
    }
    if id.contains('/') || id.contains("..") {
        return error_response(&ApiError::BadRequest("invalid media task id".into()));
    }
    let task = match g.media_tasks.lock().await.get(&id).cloned() {
        Some(task) => task,
        None => return error_response(&ApiError::NotFound("there is no such media task".into())),
    };
    // A vendor that keeps its media outside the OpenAI family may finish
    // the video straight away and have no task endpoints at all; then the
    // gateway answers from the answer it already has. A vendor that
    // follows the OpenAI family has task endpoints of its own.
    let paths = task.provider.media_paths.as_ref();
    let has_native = match kind {
        MediaKind::VideoStatus => {
            paths.and_then(|p| p.video_status.as_ref()).is_some() || paths.is_none()
        }
        MediaKind::VideoContent => {
            paths.and_then(|p| p.video_content.as_ref()).is_some() || paths.is_none()
        }
        MediaKind::VideoDelete => {
            paths.and_then(|p| p.video_delete.as_ref()).is_some() || paths.is_none()
        }
        _ => false,
    };
    if !has_native {
        return match kind {
            MediaKind::VideoStatus => Json(task.body).into_response(),
            MediaKind::VideoDelete => StatusCode::NO_CONTENT.into_response(),
            MediaKind::VideoContent => download_media_result(g, &task, &id).await,
            _ => error_response(&ApiError::NotFound("there is no such media task".into())),
        };
    }
    let url = match media_url(&task.provider, &task.target, kind, Some(&id)) {
        Ok(url) => url,
        Err(e) => return error_response(&e),
    };
    let auth = match target_headers(&task.provider, &task.target, None, false, None).await {
        Ok(headers) => headers,
        Err(e) => return error_response(&e),
    };
    let http = match g.client_for(&task.target.provider_id, &task.provider).await {
        Ok(client) => client,
        Err(e) => return error_response(&ApiError::Internal(e.to_string())),
    };
    let mut request = match method {
        "DELETE" => http.delete(url),
        _ => http.get(url),
    };
    for (name, value) in &auth {
        request = request.header(name, value);
    }
    match request.send().await {
        Ok(response) => match media_response(response).await {
            Ok(response) => response,
            Err(e) => error_response(&e),
        },
        Err(e) => error_response(&ApiError::Upstream(e.to_string())),
    }
}

/// Fetch the finished video from the url the vendor returned.
async fn download_media_result(g: &Gateway, task: &MediaTask, id: &str) -> Response {
    let Some(url) = media_result_url(&task.body) else {
        return error_response(&ApiError::NotFound(format!(
            "the video {id} has no download url yet"
        )));
    };
    let auth = match target_headers(&task.provider, &task.target, None, false, None).await {
        Ok(headers) => headers,
        Err(e) => return error_response(&e),
    };
    let http = match g.client_for(&task.target.provider_id, &task.provider).await {
        Ok(client) => client,
        Err(e) => return error_response(&ApiError::Internal(e.to_string())),
    };
    let mut request = http.get(&url);
    for (name, value) in &auth {
        request = request.header(name, value);
    }
    match request.send().await {
        Ok(response) => match media_response(response).await {
            Ok(response) => response,
            Err(e) => error_response(&e),
        },
        Err(e) => error_response(&ApiError::Upstream(e.to_string())),
    }
}

/// The download url of a finished video, wherever the vendor put it.
fn media_result_url(body: &Value) -> Option<String> {
    if let Some(url) = body.get("url").and_then(Value::as_str) {
        return Some(url.to_string());
    }
    if let Some(items) = body.get("data").and_then(Value::as_array) {
        for item in items {
            if let Some(url) = item.get("url").and_then(Value::as_str) {
                return Some(url.to_string());
            }
        }
    }
    body.pointer("/output/url")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Anthropic's token counter. The answer is an estimate; an agent only uses
/// it to learn how much room is left.
async fn count_tokens(
    State(g): State<Gateway>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    let value: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let text = value
        .get("messages")
        .and_then(Value::as_array)
        .map(|msgs| {
            msgs.iter()
                .map(|m| {
                    m.get("content")
                        .map(|c| match c {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let input_tokens = (text.chars().count() as f64 / 3.6).ceil() as u64;
    Json(json!({"input_tokens": input_tokens})).into_response()
}

/// Gemini's path carries the model and the action: `model:streamGenerateContent`.
async fn gemini_route(
    State(g): State<Gateway>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let (model, action) = match path.rsplit_once(':') {
        Some((m, a)) => (m.to_string(), a.to_string()),
        None => (path.clone(), "generateContent".to_string()),
    };
    let stream = action.contains("stream");
    // Gemini callers name the model the way the vendor does, so a bare id
    // has to be found among the providers.
    let model = {
        let config = g.state.config.read().await;
        resolve_bare_model(&config, &model)
    };
    handle_gemini(g, model, stream, headers, body).await
}

/// Find the provider that serves a model named without its provider.
fn resolve_bare_model(config: &crate::config::Config, model: &str) -> String {
    if model.contains('/') {
        return model.to_string();
    }
    let mut found: Option<String> = None;
    for p in &config.providers {
        if !p.enabled {
            continue;
        }
        if p.models.iter().any(|m| m == model) {
            match &found {
                None => found = Some(p.id.clone()),
                Some(first) => {
                    // More than one provider serves it: the first one wins,
                    // but the caller can always be explicit.
                    return format!("{first}/{model}");
                }
            }
        }
    }
    match found {
        Some(id) => format!("{id}/{model}"),
        None => model.to_string(),
    }
}

async fn handle_gemini(
    g: Gateway,
    model: String,
    stream: bool,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let mut value: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    if let Some(map) = value.as_object_mut() {
        map.insert("model".into(), json!(model));
        if stream {
            map.insert("stream".into(), json!(true));
        }
    }
    handle(
        g,
        Protocol::Gemini,
        headers,
        axum::body::Bytes::from(value.to_string()),
    )
    .await
}

/// The pipeline every request goes through.
async fn handle(
    g: Gateway,
    protocol: Protocol,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(e) = authorize(&g, &headers).await {
        return error_response(&e);
    }
    // The CLI or the web page may have changed the file.
    g.state.sync().await;
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            return error_response(&ApiError::BadRequest(format!(
                "the body is not json: {err}"
            )))
        }
    };

    let parsed = match protocol {
        Protocol::OpenAiChat => openai_chat::parse_request(&value),
        Protocol::OpenAiResponses => openai_responses::parse_request(&value),
        Protocol::Anthropic => anthropic::parse_request(&value),
        Protocol::Gemini => gemini::parse_request(&value),
        // A QwenWork request is parsed upstream and never reaches here.
        Protocol::QwenWork => {
            return error_response(&ApiError::BadRequest(
                "a QwenWork request must name a qwenwork provider".into(),
            ))
        }
    };
    let mut request = match parsed {
        Ok(r) => r,
        Err(e) => {
            return error_response(&ApiError::BadRequest(format!(
                "the request could not be read: {e}"
            )))
        }
    };

    // The agent's own session id, when it sent one: in the body like
    // Anthropic does, or a header like every other tool does.
    let session = value
        .get("metadata")
        .and_then(|m| m.get("user_id"))
        .and_then(Value::as_str)
        .or_else(|| value.get("user").and_then(Value::as_str))
        .or_else(|| {
            headers
                .get("x-session-id")
                .or_else(|| headers.get("x-magpie-session"))
                .and_then(|v| v.to_str().ok())
        })
        .map(|s| s.to_string());
    request.session_id = session.clone();

    // The context breakdown: how full this request's window is, by source.
    // Captured once here from the already-parsed request, then copied into
    // every usage record the request produces (success or each failure).
    let context = crate::gateway::usage::measure_request_context(
        &request.system,
        &request.tools,
        &request.messages,
        0,
    );
    // Remember the session so the Usage page can title it; the title is the
    // first user message's opening line, the way a chat thread is named.
    if let Some(id) = &session {
        let title = request
            .messages
            .iter()
            .find(|m| matches!(m.role, crate::protocol::ir::Role::User))
            .and_then(|m| m.text().lines().next().map(|s| s.to_string()));
        crate::sessions::note(id, title.as_deref());
    }

    // The project the request belongs to: the working directory the agent
    // ran from. Agents report it when they know it — a `metadata.user_id` of
    // the Anthropic `user_xxx__account__cwd_...` shape, a `metadata.cwd`, or
    // an `x-project` / `x-cwd` header. The first seen becomes the project's
    // key in the registry, where the user can rename and budget it.
    let project_dir = request_project_dir(&value, &headers)
        .or_else(|| session.as_deref().and_then(extract_cwd_from_session));
    request.project = project_dir.clone();
    if let Some(dir) = &project_dir {
        crate::projects::note(dir);
    }

    // Whether the request continues an exchange: any assistant or tool turn
    // means it does, and a turn-scoped affinity stays bound.
    let has_history = request.messages.iter().any(|m| {
        matches!(
            m.role,
            crate::protocol::ir::Role::Assistant | crate::protocol::ir::Role::Tool
        )
    });

    let (plan, send_agent_ua, masking) = {
        let config = g.state.config.read().await;
        match g
            .router
            .plan(&config, &request.model, session.as_deref(), has_history)
            .await
        {
            Ok(p) => (
                p,
                config.settings.send_agent_user_agent,
                config.settings.masking,
            ),
            Err(e) => return error_response(&e),
        }
    };
    // Mask secrets in the prompt before they leave the machine. The request
    // is already canonical, so this is one pass over the text blocks.
    if masking {
        mask_request_secrets(&mut request);
    }

    let asked = request.model.clone();
    let stream = request.stream;
    let agent_user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // A QwenWork (千问办公) request is built and COSY-signed by the local
    // `qwen` module; it never travels the generic OpenAI path. Route it the
    // moment a QwenWork provider is in the plan, before the generic loop.
    if plan.targets.iter().any(|t| t.provider_id == "qwenwork") {
        return qwen_attempt(
            &g,
            protocol,
            &request,
            &plan,
            agent_user_agent.as_deref(),
            &context,
        )
        .await;
    }
    if plan.targets.iter().any(|t| t.provider_id == "trae-cn") {
        return trae_attempt(
            &g,
            protocol,
            &request,
            &plan,
            agent_user_agent.as_deref(),
            &context,
        )
        .await;
    }

    let mut last_error: ApiError = ApiError::Upstream("no target could answer".into());
    for target in plan.targets.clone() {
        let provider = match g.state.config.read().await.provider(&target.provider_id) {
            Some(p) => p.clone(),
            None => continue,
        };
        // A provider that asked to be held back while its quota is empty
        // waits here instead of getting a request it cannot serve.
        if provider.hold_when_exhausted && provider.uses_session() {
            match crate::workbuddy::exhausted().await {
                Ok(true) => {
                    last_error = ApiError::RateLimited(format!(
                        "the provider {} is out of quota",
                        target.provider_id
                    ));
                    continue;
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(
                        "the quota of {} could not be read: {err}",
                        target.provider_id
                    );
                }
            }
        }
        let started = Instant::now();
        let result = if stream {
            stream_attempt(
                &g,
                protocol,
                &plan,
                &request,
                &target,
                &provider,
                agent_user_agent.as_deref(),
                send_agent_ua,
                &headers,
                &context,
            )
            .await
        } else {
            whole_attempt(
                &g,
                protocol,
                &plan,
                &request,
                &target,
                &provider,
                agent_user_agent.as_deref(),
                send_agent_ua,
                &headers,
                &context,
            )
            .await
        };
        match result {
            Ok(response) => {
                g.router.note_used(&asked).await;
                if let Some(session) = &session {
                    g.router.note_affinity(session, &target.key_id).await;
                    note_route_trace(
                        &g,
                        session,
                        &plan,
                        &asked,
                        Some(route::PlanEntry {
                            provider: target.provider_id.clone(),
                            model: target.model.clone(),
                            key: target.key_id.clone(),
                        }),
                    )
                    .await;
                }
                return response;
            }
            Err((err, mut rec)) => {
                rec.status = err.status();
                rec.ok = false;
                rec.error = Some(err.to_string());
                rec.latency_ms = started.elapsed().as_millis() as u64;
                let record = g.state.config.read().await.settings.record_usage;
                if record {
                    crate::pricing::price_record(&mut rec);
                    persist_usage(&g.state, &g.sqlite, &rec);
                }
                last_error = err;
                if !last_error.retryable() {
                    break;
                }
            }
        }
    }
    // Whatever the outcome, the routing view can say what happened.
    if let Some(session) = &session {
        note_route_trace(&g, session, &plan, &asked, None).await;
    }
    error_response(&last_error)
}

/// Write down the route a session took, for the routing view.
async fn note_route_trace(
    g: &Gateway,
    session: &str,
    plan: &route::RoutePlan,
    asked: &str,
    served: Option<route::PlanEntry>,
) {
    g.router
        .note_trace(
            session,
            route::RouteTrace {
                asked: asked.to_string(),
                plan: plan
                    .targets
                    .iter()
                    .map(|t| route::PlanEntry {
                        provider: t.provider_id.clone(),
                        model: t.model.clone(),
                        key: t.key_id.clone(),
                    })
                    .collect(),
                served,
                reason: plan.reason.clone(),
                at: chrono::Utc::now().to_rfc3339(),
            },
        )
        .await;
}

/// Write one usage record to its sinks: SQLite when it is on, and the
/// JSONL log for anyone tailing it.
fn persist_usage(state: &AppState, sqlite: &Option<store::Store>, rec: &usage::UsageRecord) {
    if let Some(store) = sqlite {
        store.record(rec);
    }
    usage::record(&state.usage_path(), rec);
}

/// The headers to send to a target, and the secret behind them.
///
/// A provider that authenticates with a local app's session has no stored
/// key: the session is read (and renewed when it is about to expire) right
/// before the request goes out.
async fn target_headers(
    provider: &Provider,
    target: &Target,
    agent_user_agent: Option<&str>,
    send_agent_ua: bool,
    caller_headers: Option<&reqwest::header::HeaderMap>,
) -> Result<BTreeMap<String, String>, ApiError> {
    if provider.uses_session() {
        // Several WorkBuddy accounts can live side by side; the requests
        // take turns so the load is shared instead of one account taking
        // all of it.
        let session = match crate::workbuddy::session_rotating().await {
            Ok((_, session)) => session,
            Err(err) => {
                // A pool of one (or none) falls back to the plain reader.
                crate::workbuddy::session()
                    .await
                    .map_err(|_| ApiError::Unauthorized(err.to_string()))?
            }
        };
        return Ok(client::session_headers(target.protocol, provider, &session));
    }
    Ok(client::auth_headers(
        target.protocol,
        provider,
        &target.secret,
        if send_agent_ua {
            agent_user_agent
        } else {
            None
        },
        caller_headers,
    ))
}

/// The request body in the protocol a vendor speaks.
fn upstream_body(request: &ChatRequest, protocol: Protocol, model: &str) -> Value {
    match protocol {
        Protocol::OpenAiChat => openai_chat::format_request(request, model),
        Protocol::OpenAiResponses => openai_responses::format_request(request, model),
        Protocol::Anthropic => anthropic::format_request(request, model),
        Protocol::Gemini => gemini::format_request(request, model),
        Protocol::QwenWork => openai_chat::format_request(request, model),
    }
}

/// The reply of a vendor in the canonical shape.
fn upstream_response(protocol: Protocol, payload: &Value) -> ApiResult<ChatResponse> {
    match protocol {
        Protocol::OpenAiChat => openai_chat::parse_response(payload),
        Protocol::OpenAiResponses => openai_responses::parse_response(payload),
        Protocol::Anthropic => anthropic::parse_response(payload),
        Protocol::Gemini => gemini::parse_response(payload),
        Protocol::QwenWork => openai_chat::parse_response(payload),
    }
}

/// The reply in the shape the caller expects.
fn downstream_response(protocol: Protocol, response: &ChatResponse, model: &str) -> Value {
    match protocol {
        Protocol::OpenAiChat => openai_chat::format_response(response, model),
        Protocol::OpenAiResponses => openai_responses::format_response(response, model),
        Protocol::Anthropic => anthropic::format_response(response, model),
        Protocol::Gemini => gemini::format_response(response, model),
        Protocol::QwenWork => openai_chat::format_response(response, model),
    }
}

/// One Trae CN request: send the caller's canonical request through the
/// Cloud-IDE SSE bridge and render it back as the caller's protocol.
async fn trae_attempt(
    g: &Gateway,
    caller_protocol: Protocol,
    request: &ChatRequest,
    plan: &RoutePlan,
    agent_user_agent: Option<&str>,
    context: &usage::ContextBreakdown,
) -> Response {
    let (account_name, session) = match crate::trae::session_rotating().await {
        Ok(pair) => pair,
        Err(err) => return error_response(&ApiError::Unauthorized(err.to_string())),
    };
    let target = match plan.targets.iter().find(|t| t.provider_id == "trae-cn") {
        Some(target) => target.clone(),
        None => return error_response(&ApiError::Upstream("no Trae CN target".into())),
    };
    let mut trae_request = request.clone();
    trae_request.model = target.model.clone();
    let mut rec = usage::UsageRecord::new(&request.model, "trae-cn");
    rec.provider = "trae-cn".to_string();
    rec.key_id = Some(account_name.clone());
    rec.key_label = Some(session.account.nickname.clone());
    rec.group = plan.group_id.clone();
    rec.session_id = request.session_id.clone();
    rec.project = request.project.clone();
    rec.agent = agent_user_agent.and_then(agent_from_user_agent);
    rec.context = Some(context.clone());
    let started = Instant::now();
    let usage_path = g.state.usage_path();
    let record = g.state.config.read().await.settings.record_usage;
    let sqlite = g.sqlite.clone();
    let events = match crate::trae::chat(&session, &trae_request).await {
        Ok(events) => events,
        Err(err) => {
            rec.ok = false;
            rec.error = Some(err.to_string());
            rec.latency_ms = started.elapsed().as_millis() as u64;
            if record {
                crate::pricing::price_record(&mut rec);
                persist_usage(&g.state, &g.sqlite, &rec);
            }
            return error_response(&ApiError::Upstream(err.to_string()));
        }
    };
    for event in &events {
        rec.note_usage(event);
    }
    if request.stream {
        events_to_stream(
            events,
            caller_protocol,
            request.model.clone(),
            rec,
            usage_path,
            record,
            started,
            sqlite,
        )
    } else {
        let parsed = ChatResponse::from_events(&events);
        Json(downstream_response(
            caller_protocol,
            &parsed,
            &request.model,
        ))
        .into_response()
    }
}

/// One QwenWork (千问办公) request: build the chat body from the caller's
/// IR, sign and send it through the local `qwen` module, and render the
/// COSY SSE reply back as the caller's protocol.
///
/// QwenWork always answers a stream, so a caller that asked for one whole
/// reply gets the events folded back together.
async fn qwen_attempt(
    g: &Gateway,
    caller_protocol: Protocol,
    request: &ChatRequest,
    plan: &RoutePlan,
    agent_user_agent: Option<&str>,
    context: &usage::ContextBreakdown,
) -> Response {
    // Several QwenWork accounts can live side by side; the requests take
    // turns so the load is shared instead of one account taking all of it.
    let session = match crate::qwen::login::session_rotating().await {
        Ok((_, session)) => session,
        Err(err) => {
            // A pool of one (or none) falls back to the plain reader.
            match crate::qwen::login::session().await {
                Ok(session) => session,
                Err(_) => return error_response(&ApiError::Unauthorized(err.to_string())),
            }
        }
    };
    let target = match plan.targets.iter().find(|t| t.provider_id == "qwenwork") {
        Some(t) => t.clone(),
        None => return error_response(&ApiError::Upstream("no QwenWork target".into())),
    };
    let model = target.model.clone();
    let body = build_qwen_body(request, &model);

    let mut rec = usage::UsageRecord::new(&request.model, "qwenwork");
    rec.provider = "qwenwork".to_string();
    rec.key_id = Some(target.key_id.clone());
    rec.key_label = target.key_label.clone();
    rec.group = plan.group_id.clone();
    rec.session_id = request.session_id.clone();
    rec.project = request.project.clone();
    rec.agent = agent_user_agent.and_then(agent_from_user_agent);
    rec.context = Some(context.clone());
    let started = Instant::now();
    let usage_path = g.state.usage_path();
    let record = g.state.config.read().await.settings.record_usage;
    let sqlite = g.sqlite.clone();

    let events = match crate::qwen::client::chat(&session, &body).await {
        Ok(events) => events,
        Err(err) => {
            let mut rec = rec;
            rec.ok = false;
            rec.error = Some(err.to_string());
            rec.latency_ms = started.elapsed().as_millis() as u64;
            if record {
                crate::pricing::price_record(&mut rec);
                if let Some(store) = &sqlite {
                    store.record(&rec);
                }
                usage::record(&usage_path, &rec);
            }
            return error_response(&ApiError::Upstream(err.to_string()));
        }
    };

    // Record usage from the final events, then render.
    for ev in &events {
        rec.note_usage(ev);
    }

    if request.stream {
        events_to_stream(
            events,
            caller_protocol,
            request.model.clone(),
            rec,
            usage_path,
            record,
            started,
            sqlite,
        )
    } else {
        let parsed = ChatResponse::from_events(&events);
        g.router.note_used(&request.model).await;
        Json(downstream_response(
            caller_protocol,
            &parsed,
            &request.model,
        ))
        .into_response()
    }
}

/// Mask secrets in the request's text before it leaves the machine.
///
/// The gateway sits between the agent and the vendor; a prompt that quotes
/// a key ("my key is sk-...") would otherwise ship verbatim. Patterns:
/// bearer-style API keys (`sk-…`, `Bearer …`), long hex strings, and
/// `KEY=value` pairs whose name says secret. Tool arguments ride along.
pub(crate) fn mask_request_secrets(request: &mut ChatRequest) {
    for block in request.system.iter_mut().chain(
        request
            .messages
            .iter_mut()
            .flat_map(|m| m.content.iter_mut()),
    ) {
        match block {
            ContentBlock::Text { text } => *text = mask_secrets(text),
            ContentBlock::ToolUse { input, .. } => mask_value_secrets(input),
            _ => {}
        }
    }
}

/// The secret patterns, as (name, regex-free matcher) described in prose.
/// Kept hand-rolled so the mask is deterministic and testable without a
/// regex dependency.
pub(crate) fn mask_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in split_keep_separators(text) {
        out.push_str(&mask_word(&word));
    }
    out
}

/// Split into words and the whitespace that joined them, so masking does
/// not reflow the prompt.
fn split_keep_separators(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_space = false;
    for c in text.chars() {
        let is_space = c.is_whitespace();
        if is_space == in_space && !current.is_empty() {
            current.push(c);
        } else if current.is_empty() {
            in_space = is_space;
            current.push(c);
        } else {
            parts.push(std::mem::take(&mut current));
            in_space = is_space;
            current.push(c);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Mask one whitespace-separated word when it looks like a secret.
fn mask_word(word: &str) -> String {
    let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if looks_like_secret(trimmed) {
        // Keep the punctuation around the word exactly where it was.
        let prefix_len = word.len()
            - word
                .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
                .len();
        let core_len = trimmed.len();
        let mut out = String::with_capacity(word.len());
        out.push_str(&word[..prefix_len]);
        out.push_str("***MASKED***");
        out.push_str(&word[prefix_len + core_len..]);
        out
    } else {
        word.to_string()
    }
}

/// Whether a token looks like a credential worth hiding.
fn looks_like_secret(token: &str) -> bool {
    if token.len() < 12 {
        return false;
    }
    // Prefixed keys: sk-, ghp_, AKIA, Bearer-quoted, AIza, xox…
    let prefixes = [
        "sk-",
        "ghp_",
        "gho_",
        "github_pat_",
        "AKIA",
        "AIza",
        "xoxb-",
        "xoxp-",
        "Bearer ",
        "bearer ",
        "ey", // JWT-ish: eyJ...
    ];
    if prefixes.iter().any(|p| token.starts_with(p)) {
        return true;
    }
    // A long hex string is usually a token or a hash.
    token.len() >= 24
        && token.chars().all(|c| c.is_ascii_hexdigit())
        && token.chars().any(|c| c.is_ascii_digit())
        && token.chars().any(|c| c.is_ascii_alphabetic())
}

/// Mask string values inside a tool's JSON arguments.
fn mask_value_secrets(value: &mut Value) {
    match value {
        Value::String(s) => *s = mask_secrets(s),
        Value::Array(items) => {
            for item in items {
                mask_value_secrets(item);
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                let name = key.to_ascii_lowercase();
                if name.contains("key")
                    || name.contains("token")
                    || name.contains("secret")
                    || name.contains("password")
                {
                    if let Value::String(s) = item {
                        if s.len() >= 6 {
                            *s = "***MASKED***".to_string();
                            continue;
                        }
                    }
                }
                mask_value_secrets(item);
            }
        }
        _ => {}
    }
}

/// Build a QwenWork chat body from the caller's normalized request.
fn build_qwen_body(request: &ChatRequest, model: &str) -> crate::qwen::client::QwenChatBody {
    let system: Vec<String> = request
        .system
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let messages: Vec<crate::qwen::client::QwenMessage> = request
        .messages
        .iter()
        .map(|m| {
            let content: Vec<crate::qwen::client::QwenContent> = match m.content.as_slice() {
                [ContentBlock::Text { text }] => {
                    vec![crate::qwen::client::QwenContent::Text(text.clone())]
                }
                [ContentBlock::Image { data, .. }] => {
                    vec![crate::qwen::client::QwenContent::Image(format!(
                        "data:image/png;base64,{data}"
                    ))]
                }
                _ => vec![],
            };
            crate::qwen::client::QwenMessage {
                role: match m.role {
                    crate::protocol::ir::Role::System => "system",
                    crate::protocol::ir::Role::User => "user",
                    crate::protocol::ir::Role::Assistant => "assistant",
                    crate::protocol::ir::Role::Tool => "tool",
                }
                .to_string(),
                content,
            }
        })
        .collect();
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|t| {
            json!({
                "function": {
                    "name": t.name,
                    "description": t.description.clone().unwrap_or_default(),
                    "parameters": t.input_schema,
                }
            })
        })
        .collect();
    crate::qwen::client::QwenChatBody {
        model: model.to_string(),
        messages,
        system,
        tools,
        max_tokens: request.max_tokens,
        enable_thinking: request
            .reasoning_effort
            .map(|e| !matches!(e, ReasoningEffort::None))
            .unwrap_or(false),
    }
}

/// One non-streaming attempt at a target.
#[allow(clippy::too_many_arguments)]
async fn whole_attempt(
    g: &Gateway,
    caller_protocol: Protocol,
    plan: &RoutePlan,
    request: &ChatRequest,
    target: &Target,
    provider: &Provider,
    agent_user_agent: Option<&str>,
    send_agent_ua: bool,
    caller_headers: &reqwest::header::HeaderMap,
    context: &usage::ContextBreakdown,
) -> Result<Response, (ApiError, usage::UsageRecord)> {
    let mut rec = usage::UsageRecord::new(&request.model, target.protocol.as_str());
    rec.provider = target.provider_id.clone();
    rec.key_id = Some(target.key_id.clone());
    rec.key_label = target.key_label.clone();
    rec.group = plan.group_id.clone();
    rec.session_id = request.session_id.clone();
    rec.project = request.project.clone();
    rec.agent = agent_user_agent.and_then(agent_from_user_agent);
    rec.context = Some(context.clone());

    let mut body = upstream_body(request, target.protocol, &target.model);
    // A vendor that only answers streaming requests is asked for a stream
    // even when the caller wants one whole reply.
    let stream_only = provider.stream_only;
    if stream_only {
        body["stream"] = json!(true);
        body["stream_options"] = json!({"include_usage": true});
    }
    let headers = match target_headers(
        provider,
        target,
        agent_user_agent,
        send_agent_ua,
        Some(caller_headers),
    )
    .await
    {
        Ok(h) => h,
        Err(e) => return Err((e, rec)),
    };

    let client = match g.client_for(&target.provider_id, provider).await {
        Ok(c) => c,
        Err(err) => return Err((ApiError::Internal(err.to_string()), rec)),
    };
    let started = Instant::now();
    let reply = match client::send(&client, target, provider, body, headers, stream_only).await {
        Ok(r) => r,
        Err(err) => {
            rec.latency_ms = started.elapsed().as_millis() as u64;
            return Err((ApiError::Upstream(err.to_string()), rec));
        }
    };
    let (status, payload) = match reply {
        UpstreamReply::Whole { status, body } => (status, body),
        UpstreamReply::Stream { status, bytes } => {
            // The vendor only speaks streaming: fold the events back into
            // one reply for the caller.
            let events = collect_stream(bytes, target.protocol).await;
            let parsed = ChatResponse::from_events(&events);
            rec.input_tokens = parsed.usage.input_tokens;
            rec.output_tokens = parsed.usage.output_tokens;
            rec.cache_read_tokens = parsed.usage.cache_read_tokens;
            rec.cache_write_tokens = parsed.usage.cache_write_tokens;
            rec.reasoning_tokens = parsed.usage.reasoning_tokens;
            rec.latency_ms = started.elapsed().as_millis() as u64;
            rec.status = status;
            crate::pricing::price_record(&mut rec);
            let outbound = downstream_response(caller_protocol, &parsed, &request.model);
            let record = g.state.config.read().await.settings.record_usage;
            if record {
                persist_usage(&g.state, &g.sqlite, &rec);
            }
            return Ok(Json(outbound).into_response());
        }
    };
    if !(200..300).contains(&status) {
        rec.latency_ms = started.elapsed().as_millis() as u64;
        let message = error_message(&payload);
        let err = match status {
            401 | 403 => ApiError::Unauthorized(message),
            404 => ApiError::NotFound(message),
            429 => ApiError::RateLimited(message),
            400..=499 => ApiError::BadRequest(message),
            _ => ApiError::Upstream(message),
        };
        return Err((err, rec));
    }

    let parsed = match upstream_response(target.protocol, &payload) {
        Ok(p) => p,
        Err(e) => {
            rec.latency_ms = started.elapsed().as_millis() as u64;
            return Err((e, rec));
        }
    };

    rec.input_tokens = parsed.usage.input_tokens;
    rec.output_tokens = parsed.usage.output_tokens;
    rec.cache_read_tokens = parsed.usage.cache_read_tokens;
    rec.cache_write_tokens = parsed.usage.cache_write_tokens;
    rec.reasoning_tokens = parsed.usage.reasoning_tokens;
    rec.latency_ms = started.elapsed().as_millis() as u64;
    rec.status = status;

    let outbound = downstream_response(caller_protocol, &parsed, &request.model);

    let record = g.state.config.read().await.settings.record_usage;
    if record {
        persist_usage(&g.state, &g.sqlite, &rec);
    }
    Ok(Json(outbound).into_response())
}

/// One streaming attempt at a target.
#[allow(clippy::too_many_arguments)]
async fn stream_attempt(
    g: &Gateway,
    caller_protocol: Protocol,
    plan: &RoutePlan,
    request: &ChatRequest,
    target: &Target,
    provider: &Provider,
    agent_user_agent: Option<&str>,
    send_agent_ua: bool,
    caller_headers: &reqwest::header::HeaderMap,
    context: &usage::ContextBreakdown,
) -> Result<Response, (ApiError, usage::UsageRecord)> {
    let mut rec = usage::UsageRecord::new(&request.model, target.protocol.as_str());
    rec.provider = target.provider_id.clone();
    rec.key_id = Some(target.key_id.clone());
    rec.key_label = target.key_label.clone();
    rec.group = plan.group_id.clone();
    rec.session_id = request.session_id.clone();
    rec.project = request.project.clone();
    rec.agent = agent_user_agent.and_then(agent_from_user_agent);
    rec.context = Some(context.clone());

    let mut body = upstream_body(request, target.protocol, &target.model);
    if provider.stream_only {
        body["stream"] = json!(true);
        body["stream_options"] = json!({"include_usage": true});
    }
    let headers = match target_headers(
        provider,
        target,
        agent_user_agent,
        send_agent_ua,
        Some(caller_headers),
    )
    .await
    {
        Ok(h) => h,
        Err(e) => return Err((e, rec)),
    };
    let client = match g.client_for(&target.provider_id, provider).await {
        Ok(c) => c,
        Err(err) => return Err((ApiError::Internal(err.to_string()), rec)),
    };
    let started = Instant::now();
    let reply = match client::send(&client, target, provider, body, headers, true).await {
        Ok(r) => r,
        Err(err) => {
            rec.latency_ms = started.elapsed().as_millis() as u64;
            return Err((ApiError::Upstream(err.to_string()), rec));
        }
    };
    match reply {
        UpstreamReply::Stream { status, bytes } => {
            if !(200..300).contains(&status) {
                return Err((
                    ApiError::Upstream("the upstream refused the stream".into()),
                    rec,
                ));
            }
            rec.status = status;
            crate::pricing::price_record(&mut rec);
            let usage_path = g.state.usage_path();
            let record_usage = g.state.config.read().await.settings.record_usage;
            Ok(build_stream(
                bytes,
                target.protocol,
                caller_protocol,
                request.model.clone(),
                rec,
                usage_path,
                record_usage,
                started,
                g.sqlite.clone(),
            ))
        }
        UpstreamReply::Whole { status, body } => {
            if !(200..300).contains(&status) {
                return Err((
                    if status == 429 {
                        ApiError::RateLimited(error_message(&body))
                    } else {
                        ApiError::Upstream(error_message(&body))
                    },
                    rec,
                ));
            }
            // The vendor answered whole even though we asked it to stream:
            // turn the reply into events and send them out as a stream.
            let parsed = match upstream_response(target.protocol, &body) {
                Ok(p) => p,
                Err(e) => return Err((e, rec)),
            };
            rec.input_tokens = parsed.usage.input_tokens;
            rec.output_tokens = parsed.usage.output_tokens;
            rec.cache_read_tokens = parsed.usage.cache_read_tokens;
            rec.cache_write_tokens = parsed.usage.cache_write_tokens;
            rec.reasoning_tokens = parsed.usage.reasoning_tokens;
            rec.latency_ms = started.elapsed().as_millis() as u64;
            rec.status = status;
            crate::pricing::price_record(&mut rec);

            let mut events = vec![StreamEvent::MessageStart {
                id: parsed.id.clone(),
                model: request.model.clone(),
                usage: parsed.usage,
            }];
            let mut index = 0usize;
            let mut text_open = false;
            for block in &parsed.content {
                match block {
                    ContentBlock::Text { text } => {
                        if !text.is_empty() {
                            events.push(StreamEvent::ContentBlockStart {
                                index,
                                kind: BlockKind::Text,
                            });
                            events.push(StreamEvent::TextDelta {
                                index,
                                text: text.clone(),
                            });
                            events.push(StreamEvent::ContentBlockStop { index });
                            index += 1;
                            text_open = false;
                        }
                    }
                    ContentBlock::Thinking { text, signature } => {
                        if !text.is_empty() {
                            events.push(StreamEvent::ContentBlockStart {
                                index,
                                kind: BlockKind::Thinking,
                            });
                            events.push(StreamEvent::ThinkingDelta {
                                index,
                                text: text.clone(),
                            });
                            if let Some(sig) = signature {
                                events.push(StreamEvent::ThinkingSignature {
                                    index,
                                    signature: sig.clone(),
                                });
                            }
                            events.push(StreamEvent::ContentBlockStop { index });
                            index += 1;
                        }
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        events.push(StreamEvent::ContentBlockStart {
                            index,
                            kind: BlockKind::ToolUse,
                        });
                        events.push(StreamEvent::ToolUseStart {
                            index,
                            id: id.clone(),
                            name: name.clone(),
                        });
                        events.push(StreamEvent::ToolInputDelta {
                            index,
                            partial_json: serde_json::to_string(input)
                                .unwrap_or_else(|_| "{}".into()),
                        });
                        events.push(StreamEvent::ContentBlockStop { index });
                        index += 1;
                    }
                    _ => {}
                }
            }
            let _ = text_open;
            events.push(StreamEvent::MessageDelta {
                stop_reason: parsed.stop_reason,
                usage: parsed.usage,
            });
            events.push(StreamEvent::MessageStop);

            let usage_path = g.state.usage_path();
            let record_usage = g.state.config.read().await.settings.record_usage;
            Ok(events_to_stream(
                events,
                caller_protocol,
                request.model.clone(),
                rec,
                usage_path,
                record_usage,
                started,
                g.sqlite.clone(),
            ))
        }
    }
}

/// Read a vendor's stream all the way to the end, collecting its events.
async fn collect_stream(mut upstream: client::ByteStream, protocol: Protocol) -> Vec<StreamEvent> {
    let mut collector: Box<dyn client::StreamCollector> = match protocol {
        Protocol::OpenAiChat => Box::new(openai_chat::Collector::new()),
        Protocol::OpenAiResponses => Box::new(openai_responses::Collector::new()),
        Protocol::Anthropic => Box::new(anthropic::Collector::new()),
        Protocol::Gemini => Box::new(gemini::Collector::new()),
        Protocol::QwenWork => Box::new(openai_chat::Collector::new()),
    };
    let mut events: Vec<StreamEvent> = Vec::new();
    let mut buffer = String::new();
    while let Some(item) = upstream.next().await {
        let Ok(chunk) = item else { break };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = client::find_frame_end(&buffer) {
            let frame = buffer[..pos.0].to_string();
            buffer = buffer[pos.1..].to_string();
            if let Some(value) = client::parse_frame(&frame) {
                collector.push(&value, &mut events);
            }
        }
    }
    collector.finish(&mut events);
    events
}

/// Writes a stream's usage record when the stream ends — or when it is
/// dropped, which is what a client that hangs up mid-reply does.
struct UsageGuard {
    rec: usage::UsageRecord,
    path: PathBuf,
    record: bool,
    started: Instant,
    done: bool,
    sqlite: Option<store::Store>,
}

impl UsageGuard {
    fn finish(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        self.rec.latency_ms = self.started.elapsed().as_millis() as u64;
        if self.record {
            // Pricing runs at write time: the tokens only arrived with the
            // stream's last events.
            crate::pricing::price_record(&mut self.rec);
            if let Some(store) = &self.sqlite {
                store.record(&self.rec);
            }
            usage::record(&self.path, &self.rec);
        }
    }
}

impl Drop for UsageGuard {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Send already-known events out as the caller's stream.
#[allow(clippy::too_many_arguments)]
fn events_to_stream(
    events: Vec<StreamEvent>,
    caller_protocol: Protocol,
    asked_model: String,
    rec: usage::UsageRecord,
    usage_path: PathBuf,
    record_usage: bool,
    started: Instant,
    sqlite: Option<store::Store>,
) -> Response {
    let id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    let body_stream = async_stream::stream! {
        let mut guard = UsageGuard {
            rec,
            path: usage_path,
            record: record_usage,
            started,
            done: false,
            sqlite,
        };
        let mut renderer = make_renderer(caller_protocol, id.clone(), asked_model.clone());
        for ev in &events {
            for out in renderer(ev) {
                yield Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(out.encode()));
            }
        }
        guard.finish();
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body_stream))
        .unwrap()
}

/// Turn a vendor's byte stream into the caller's event stream.
#[allow(clippy::too_many_arguments)]
fn build_stream(
    mut upstream: client::ByteStream,
    upstream_protocol: Protocol,
    caller_protocol: Protocol,
    asked_model: String,
    rec: usage::UsageRecord,
    usage_path: PathBuf,
    record_usage: bool,
    started: Instant,
    sqlite: Option<store::Store>,
) -> Response {
    let id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    let body_stream = async_stream::stream! {
        let mut guard = UsageGuard {
            rec,
            path: usage_path,
            record: record_usage,
            started,
            done: false,
            sqlite,
        };
        let mut collector: Box<dyn client::StreamCollector> = match upstream_protocol {
            Protocol::OpenAiChat => Box::new(openai_chat::Collector::new()),
            Protocol::OpenAiResponses => Box::new(openai_responses::Collector::new()),
            Protocol::Anthropic => Box::new(anthropic::Collector::new()),
            Protocol::Gemini => Box::new(gemini::Collector::new()),
            Protocol::QwenWork => Box::new(openai_chat::Collector::new()),
        };
        let mut renderer = make_renderer(caller_protocol, id.clone(), asked_model.clone());
        let mut events: Vec<StreamEvent> = Vec::new();
        let mut buffer = String::new();

        while let Some(item) = upstream.next().await {
            let chunk = match item {
                Ok(c) => c,
                Err(err) => {
                    guard.rec.ok = false;
                    guard.rec.error = Some(err.to_string());
                    break;
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = client::find_frame_end(&buffer) {
                let frame = buffer[..pos.0].to_string();
                buffer = buffer[pos.1..].to_string();
                if let Some(value) = client::parse_frame(&frame) {
                    collector.push(&value, &mut events);
                    for ev in events.drain(..) {
                        guard.rec.note_usage(&ev);
                        for out in renderer(&ev) {
                            yield bytes::Bytes::from(out.encode());
                        }
                    }
                }
            }
        }
        collector.finish(&mut events);
        for ev in events.drain(..) {
            guard.rec.note_usage(&ev);
            for out in renderer(&ev) {
                yield bytes::Bytes::from(out.encode());
            }
        }

        guard.finish();
    }
    .map(Ok::<bytes::Bytes, std::io::Error>);

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body_stream))
        .unwrap()
}

/// The frames a renderer produces for one canonical event.
pub type Rendered = Box<dyn FnMut(&StreamEvent) -> Vec<sse::SseFrame> + Send>;

/// A renderer for the protocol the caller speaks.
#[allow(clippy::type_complexity)]
fn make_renderer(protocol: Protocol, id: String, model: String) -> Rendered {
    match protocol {
        Protocol::OpenAiChat => {
            let mut r = openai_chat::Renderer::new(id, model);
            Box::new(move |ev| r.render(ev))
        }
        Protocol::OpenAiResponses => {
            let mut r = openai_responses::Renderer::new(id, model);
            Box::new(move |ev| r.render(ev))
        }
        Protocol::Anthropic => {
            let mut r = anthropic::Renderer::new(id, model);
            Box::new(move |ev| r.render(ev))
        }
        Protocol::Gemini => {
            let mut r = gemini::Renderer::new(id, model);
            Box::new(move |ev| r.render(ev))
        }
        Protocol::QwenWork => {
            let mut r = openai_chat::Renderer::new(id, model);
            Box::new(move |ev| r.render(ev))
        }
    }
}

/// The message a vendor's error carries.
fn error_message(payload: &Value) -> String {
    payload
        .get("error")
        .and_then(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .or_else(|| e.as_str())
        })
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get("message")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "the vendor refused the request".into())
}

/// Turn an error into the shape the caller expects.
fn error_response(err: &ApiError) -> Response {
    let status = StatusCode::from_u16(err.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = match err {
        ApiError::Unauthorized(_) => json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": err.to_string()},
        }),
        ApiError::NotFound(_) => json!({
            "type": "error",
            "error": {"type": "not_found_error", "message": err.to_string()},
        }),
        ApiError::RateLimited(_) => json!({
            "type": "error",
            "error": {"type": "rate_limit_error", "message": err.to_string()},
        }),
        ApiError::BadRequest(_) => json!({
            "type": "error",
            "error": {"type": "invalid_request_error", "message": err.to_string()},
        }),
        _ => json!({
            "type": "error",
            "error": {"type": "api_error", "message": err.to_string()},
        }),
    };
    (status, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_api_keys_are_masked() {
        let masked = mask_secrets("my key is sk-abcdef1234567890abcdef keep it safe");
        assert!(!masked.contains("sk-abcdef1234567890abcdef"));
        assert!(masked.contains("***MASKED***"));
        assert!(masked.starts_with("my key is"));
        assert!(masked.ends_with("keep it safe"));
        // Short words and normal prose survive.
        assert_eq!(mask_secrets("hello world"), "hello world");
    }

    #[test]
    fn hex_tokens_and_bearer_prefixes_are_masked() {
        let hex = mask_secrets("token deadbeefdeadbeefdeadbeef12 in config");
        assert!(hex.contains("***MASKED***"), "{hex}");
        assert!(!hex.contains("deadbeefdeadbeefdeadbeef12"));
        let bearer = mask_secrets("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9");
        assert!(bearer.contains("***MASKED***"), "{bearer}");
    }

    #[test]
    fn ordinary_hex_and_prose_are_left_alone() {
        // Short hex (a color, a short hash) is not a secret.
        assert_eq!(
            mask_secrets("color #deadbeef size 42"),
            "color #deadbeef size 42"
        );
        // A long decimal number (a timestamp, an id) is not a secret.
        let digits = "123456789012345678901234";
        assert_eq!(mask_secrets(digits), digits);
        assert_eq!(mask_secrets("the quick brown fox"), "the quick brown fox");
    }

    #[test]
    fn tool_arguments_with_secret_names_are_masked() {
        let mut request = ChatRequest {
            messages: vec![Message::user("run this")],
            tools: Vec::new(),
            ..ChatRequest::default()
        };
        request.messages[0].content.push(ContentBlock::ToolUse {
            id: "c1".into(),
            name: "set_env".into(),
            input: serde_json::json!({
                "api_key": "super-secret-value-123",
                "path": "/tmp/work",
                "nested": {"token": "abcdefgh1234", "keep": "visible"}
            }),
        });
        mask_request_secrets(&mut request);
        if let Some(ContentBlock::ToolUse { input, .. }) = &request.messages[0].content.last() {
            assert_eq!(input["api_key"], "***MASKED***");
            assert_eq!(input["nested"]["token"], "***MASKED***");
            assert_eq!(input["nested"]["keep"], "visible");
            assert_eq!(input["path"], "/tmp/work");
        } else {
            panic!("the tool block vanished");
        }
        // The prose survives untouched.
        assert_eq!(request.messages[0].text(), "run this");
    }

    #[test]
    fn system_and_text_blocks_are_both_covered() {
        let mut request = ChatRequest {
            system: vec![ContentBlock::text("use sk-abcdef1234567890abcdef")],
            messages: vec![Message::user("see ghp_abcdefghijklmnopqrstuvwxyz0")],
            ..ChatRequest::default()
        };
        mask_request_secrets(&mut request);
        assert!(request.system[0].plain_text().contains("***MASKED***"));
        assert!(request.messages[0].text().contains("***MASKED***"));
    }
}
