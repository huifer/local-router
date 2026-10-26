//! The gateway: the HTTP surface every agent talks to.

pub mod client;
pub mod route;
pub mod usage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::config::{AppState, Provider};
use crate::protocol::ir::*;
use crate::protocol::{anthropic, gemini, openai_chat, openai_responses, sse};

use client::{Target, UpstreamReply};
use route::{RoutePlan, Router};

/// Everything a gateway handler needs.
#[derive(Clone)]
pub struct Gateway {
    pub state: AppState,
    pub router: Arc<Router>,
    pub clients: Arc<Mutex<HashMap<String, reqwest::Client>>>,
}

impl Gateway {
    pub fn new(state: AppState) -> Self {
        Gateway {
            state,
            router: Arc::new(Router::new()),
            clients: Arc::new(Mutex::new(HashMap::new())),
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
    axum::Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/models/{provider}/{model}", get(get_model))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/responses", post(responses))
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens))
        .route("/v1beta/models/{*model}", post(gemini_route))
        .route("/v1/local-router/route", get(route_info))
        .route("/v1/local-router/status", get(status))
        .with_state(gateway)
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
    if let Err(e) = check_auth(&g, &headers) {
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
    if let Err(e) = check_auth(&g, &headers) {
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
async fn route_info(State(g): State<Gateway>) -> Json<Value> {
    let config = g.state.config.read().await;
    let groups: Vec<Value> = config
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
        .collect();
    Json(json!({"groups": groups}))
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

/// Anthropic's token counter. The answer is an estimate; an agent only uses
/// it to learn how much room is left.
async fn count_tokens(
    State(g): State<Gateway>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(e) = check_auth(&g, &headers) {
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
    if let Err(e) = check_auth(&g, &headers) {
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
    };
    let mut request = match parsed {
        Ok(r) => r,
        Err(e) => {
            return error_response(&ApiError::BadRequest(format!(
                "the request could not be read: {e}"
            )))
        }
    };

    // The agent's own session id, when it sent one.
    let session = value
        .get("metadata")
        .and_then(|m| m.get("user_id"))
        .and_then(Value::as_str)
        .or_else(|| value.get("user").and_then(Value::as_str))
        .map(|s| s.to_string());
    request.session_id = session.clone();

    let (plan, send_agent_ua) = {
        let config = g.state.config.read().await;
        match g
            .router
            .plan(&config, &request.model, session.as_deref())
            .await
        {
            Ok(p) => (p, config.settings.send_agent_user_agent),
            Err(e) => return error_response(&e),
        }
    };

    let asked = request.model.clone();
    let stream = request.stream;
    let agent_user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let mut last_error: ApiError = ApiError::Upstream("no target could answer".into());
    for target in plan.targets.clone() {
        let provider = match g.state.config.read().await.provider(&target.provider_id) {
            Some(p) => p.clone(),
            None => continue,
        };
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
            )
            .await
        };
        match result {
            Ok(response) => {
                g.router.note_used(&asked).await;
                if let Some(session) = &session {
                    g.router.note_affinity(session, &target.key_id).await;
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
                    usage::record(&crate::config::usage_file(), &rec);
                }
                last_error = err;
                if !last_error.retryable() {
                    break;
                }
            }
        }
    }
    error_response(&last_error)
}

/// The request body in the protocol a vendor speaks.
fn upstream_body(request: &ChatRequest, protocol: Protocol, model: &str) -> Value {
    match protocol {
        Protocol::OpenAiChat => openai_chat::format_request(request, model),
        Protocol::OpenAiResponses => openai_responses::format_request(request, model),
        Protocol::Anthropic => anthropic::format_request(request, model),
        Protocol::Gemini => gemini::format_request(request, model),
    }
}

/// The reply of a vendor in the canonical shape.
fn upstream_response(protocol: Protocol, payload: &Value) -> ApiResult<ChatResponse> {
    match protocol {
        Protocol::OpenAiChat => openai_chat::parse_response(payload),
        Protocol::OpenAiResponses => openai_responses::parse_response(payload),
        Protocol::Anthropic => anthropic::parse_response(payload),
        Protocol::Gemini => gemini::parse_response(payload),
    }
}

/// The reply in the shape the caller expects.
fn downstream_response(protocol: Protocol, response: &ChatResponse, model: &str) -> Value {
    match protocol {
        Protocol::OpenAiChat => openai_chat::format_response(response, model),
        Protocol::OpenAiResponses => openai_responses::format_response(response, model),
        Protocol::Anthropic => anthropic::format_response(response, model),
        Protocol::Gemini => gemini::format_response(response, model),
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
) -> Result<Response, (ApiError, usage::UsageRecord)> {
    let mut rec = usage::UsageRecord::new(&request.model, target.protocol.as_str());
    rec.provider = target.provider_id.clone();
    rec.key_id = Some(target.key_id.clone());
    rec.key_label = target.key_label.clone();
    rec.group = plan.group_id.clone();

    let body = upstream_body(request, target.protocol, &target.model);
    let headers = client::auth_headers(
        target.protocol,
        provider,
        &target.secret,
        if send_agent_ua {
            agent_user_agent
        } else {
            None
        },
    );

    let client = match g.client_for(&target.provider_id, provider).await {
        Ok(c) => c,
        Err(err) => return Err((ApiError::Internal(err.to_string()), rec)),
    };
    let started = Instant::now();
    let reply = match client::send(&client, target, provider, body, headers, false).await {
        Ok(r) => r,
        Err(err) => {
            rec.latency_ms = started.elapsed().as_millis() as u64;
            return Err((ApiError::Upstream(err.to_string()), rec));
        }
    };
    let (status, payload) = match reply {
        UpstreamReply::Whole { status, body } => (status, body),
        UpstreamReply::Stream { .. } => {
            return Err((
                ApiError::Internal("the upstream streamed unexpectedly".into()),
                rec,
            ))
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
        usage::record(&crate::config::usage_file(), &rec);
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
) -> Result<Response, (ApiError, usage::UsageRecord)> {
    let mut rec = usage::UsageRecord::new(&request.model, target.protocol.as_str());
    rec.provider = target.provider_id.clone();
    rec.key_id = Some(target.key_id.clone());
    rec.key_label = target.key_label.clone();
    rec.group = plan.group_id.clone();

    let body = upstream_body(request, target.protocol, &target.model);
    let headers = client::auth_headers(
        target.protocol,
        provider,
        &target.secret,
        if send_agent_ua {
            agent_user_agent
        } else {
            None
        },
    );
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
            let usage_path = crate::config::usage_file();
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

            let usage_path = crate::config::usage_file();
            let record_usage = g.state.config.read().await.settings.record_usage;
            Ok(events_to_stream(
                events,
                caller_protocol,
                request.model.clone(),
                rec,
                usage_path,
                record_usage,
                started,
            ))
        }
    }
}

/// Writes a stream's usage record when the stream ends — or when it is
/// dropped, which is what a client that hangs up mid-reply does.
struct UsageGuard {
    rec: usage::UsageRecord,
    path: PathBuf,
    record: bool,
    started: Instant,
    done: bool,
}

impl UsageGuard {
    fn finish(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        self.rec.latency_ms = self.started.elapsed().as_millis() as u64;
        if self.record {
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
) -> Response {
    let id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    let body_stream = async_stream::stream! {
        let mut guard = UsageGuard { rec, path: usage_path, record: record_usage, started, done: false };
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
) -> Response {
    let id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    let body_stream = async_stream::stream! {
        let mut guard = UsageGuard { rec, path: usage_path, record: record_usage, started, done: false };
        let mut collector: Box<dyn client::StreamCollector> = match upstream_protocol {
            Protocol::OpenAiChat => Box::new(openai_chat::Collector::new()),
            Protocol::OpenAiResponses => Box::new(openai_responses::Collector::new()),
            Protocol::Anthropic => Box::new(anthropic::Collector::new()),
            Protocol::Gemini => Box::new(gemini::Collector::new()),
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
                        for out in renderer(&ev) {
                            yield bytes::Bytes::from(out.encode());
                        }
                    }
                }
            }
        }
        collector.finish(&mut events);
        for ev in events.drain(..) {
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
