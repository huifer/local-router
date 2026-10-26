//! The web page and the API behind it.

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::config::{AppState, Group, Provider, ProviderKey, Routing, Settings, Stays};
use crate::gateway::client;
use crate::gateway::usage;
use crate::presets;

const INDEX_HTML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/dist/index.html"));
const APP_JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/dist/assets/app.js"));
const STYLE_CSS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/dist/assets/style.css"));

/// Build the web server's routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/app.js", get(app_js))
        .route("/assets/style.css", get(style_css))
        .route("/api/status", get(api_status))
        .route("/api/presets", get(api_presets))
        .route("/api/providers", get(api_providers).post(api_add_provider))
        .route(
            "/api/providers/{id}",
            patch(api_update_provider).delete(api_delete_provider),
        )
        .route("/api/providers/{id}/keys", post(api_add_key))
        .route(
            "/api/providers/{id}/keys/{key_id}",
            delete(api_delete_key),
        )
        .route("/api/providers/{id}/models", post(api_set_models))
        .route("/api/providers/{id}/refresh", post(api_refresh_models))
        .route("/api/providers/{id}/test", post(api_test_provider))
        .route("/api/models", get(api_models))
        .route("/api/groups", get(api_groups).post(api_add_group))
        .route(
            "/api/groups/{id}",
            patch(api_update_group).delete(api_delete_group),
        )
        .route("/api/settings", get(api_settings).put(api_save_settings))
        .route("/api/caller-keys", get(api_caller_keys).post(api_add_caller_key))
        .route("/api/caller-keys/{id}", delete(api_delete_caller_key))
        .route("/api/usage", get(api_usage))
        .with_state(state)
}

// ------------------------------------------------------------------ pages --

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn app_js() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        APP_JS,
    )
}

async fn style_css() -> impl IntoResponse {
    ([(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")], STYLE_CSS)
}

// ------------------------------------------------------------------- api --

async fn api_status(State(state): State<AppState>) -> Json<Value> {
    let config = state.config.read().await;
    Json(json!({
        "version": client::VERSION,
        "providers": config.providers.len(),
        "groups": config.groups.len(),
        "models": config.model_ids().len(),
        "settings": config.settings,
    }))
}

async fn api_presets() -> Json<Value> {
    let grouped = presets::grouped();
    let out: Vec<Value> = grouped
        .into_iter()
        .map(|(group, list)| {
            json!({
                "group": group,
                "presets": list.iter().map(|p| json!({
                    "id": p.id,
                    "name": p.name,
                    "protocol": p.protocol,
                    "base_url": p.base_url,
                    "catalog": p.catalog,
                    "default_models": p.default_models,
                    "key_hint": p.key_hint,
                    "docs": p.docs,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(json!({"presets": out}))
}

/// A provider as the browser may see it: the keys are masked.
fn redact(provider: &Provider) -> Value {
    let mut p = provider.clone();
    p.keys = provider
        .keys
        .iter()
        .map(|k| ProviderKey {
            id: k.id.clone(),
            key: mask(&k.key),
            enabled: k.enabled,
            label: k.label.clone(),
        })
        .collect();
    serde_json::to_value(p).unwrap_or(json!({}))
}

fn mask(key: &str) -> String {
    if key.len() <= 8 {
        "••••".to_string()
    } else {
        format!("{}…{}", &key[..4], &key[key.len() - 4..])
    }
}

async fn api_providers(State(state): State<AppState>) -> Json<Value> {
    let config = state.config.read().await;
    Json(json!({
        "providers": config.providers.iter().map(redact).collect::<Vec<_>>(),
    }))
}

#[derive(serde::Deserialize)]
struct AddProvider {
    /// A preset id, or `custom`.
    #[serde(default)]
    preset: Option<String>,
    name: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    anthropic_base_url: Option<String>,
    #[serde(default)]
    protocol: Option<crate::protocol::ir::Protocol>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    models: Vec<String>,
}

async fn api_add_provider(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let input: AddProvider = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("the body is not json: {err}")})),
            ))
        }
    };

    let provider = match input.preset.as_deref() {
        Some(preset_id) if preset_id != "custom" => match presets::get(preset_id) {
            Some(preset) => {
                let mut p = presets::to_provider(preset, input.key.as_deref());
                if let Some(url) = &input.base_url {
                    p.base_url = Some(url.trim_end_matches('/').to_string());
                }
                if let Some(url) = &input.anthropic_base_url {
                    p.anthropic_base_url = Some(url.trim_end_matches('/').to_string());
                }
                if !input.models.is_empty() {
                    p.models = input.models.clone();
                }
                p
            }
            None => {
                // Not a preset: a provider the user described by hand.
                let protocol = input
                    .protocol
                    .unwrap_or(crate::protocol::ir::Protocol::OpenAiChat);
                let mut p = crate::presets::custom_provider(
                    &input.name,
                    input.base_url.as_deref(),
                    input.anthropic_base_url.as_deref(),
                    protocol,
                    input.key.as_deref(),
                    &input.models,
                );
                if let Some(id) = &input.id {
                    p.id = id.trim().to_string();
                }
                p
            }
        },
        _ => {
            let protocol = input.protocol.unwrap_or(crate::protocol::ir::Protocol::OpenAiChat);
            let mut p = crate::presets::custom_provider(
                &input.name,
                input.base_url.as_deref(),
                input.anthropic_base_url.as_deref(),
                protocol,
                input.key.as_deref(),
                &input.models,
            );
            if let Some(id) = &input.id {
                p.id = id.trim().to_string();
            }
            p
        }
    };

    let id = provider.id.clone();
    let name = provider.name.clone();
    state
        .update(|config| {
            if config.providers.iter().any(|p| p.id == id) {
                return Err("there is already a provider with that id".to_string());
            }
            config.providers.push(provider);
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::CONFLICT, Json(json!({"error": e}))))?;

    Ok(Json(json!({"ok": true, "id": id, "name": name})))
}

#[derive(serde::Deserialize)]
struct UpdateProvider {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    anthropic_base_url: Option<String>,
    #[serde(default)]
    protocol: Option<crate::protocol::ir::Protocol>,
    #[serde(default)]
    models: Option<Vec<String>>,
    #[serde(default)]
    auto_include: Option<Vec<String>>,
    #[serde(default)]
    group_only: Option<bool>,
}

async fn api_update_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let input: UpdateProvider = serde_json::from_slice(&body).unwrap_or(UpdateProvider {
        name: None,
        enabled: None,
        base_url: None,
        anthropic_base_url: None,
        protocol: None,
        models: None,
        auto_include: None,
        group_only: None,
    });
    state
        .update(|config| {
            let Some(p) = config.provider_mut(&id) else {
                return Err("there is no provider with that id".to_string());
            };
            if let Some(v) = input.name {
                p.name = v;
            }
            if let Some(v) = input.enabled {
                p.enabled = v;
            }
            if let Some(v) = input.base_url {
                p.base_url = Some(v.trim_end_matches('/').to_string());
            }
            if let Some(v) = input.anthropic_base_url {
                p.anthropic_base_url = Some(v.trim_end_matches('/').to_string());
            }
            if let Some(v) = input.protocol {
                p.protocol = v;
            }
            if let Some(v) = input.models {
                p.models = v;
            }
            if let Some(v) = input.auto_include {
                p.auto_include = v;
            }
            if let Some(v) = input.group_only {
                p.group_only = v;
            }
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::NOT_FOUND, Json(json!({"error": e}))))?;
    Ok(Json(json!({"ok": true})))
}

async fn api_delete_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .update(|config| {
            let before = config.providers.len();
            config.providers.retain(|p| p.id != id);
            if config.providers.len() == before {
                return Err("there is no provider with that id".to_string());
            }
            for group in &mut config.groups {
                group.models.retain(|m| !m.starts_with(&format!("{id}/")));
            }
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::NOT_FOUND, Json(json!({"error": e}))))?;
    Ok(Json(json!({"ok": true})))
}

#[derive(serde::Deserialize)]
struct AddKey {
    key: String,
    #[serde(default)]
    label: Option<String>,
}

async fn api_add_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let input: AddKey = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("the body is not json: {e}")})),
        )
    })?;
    state
        .update(|config| {
            let Some(p) = config.provider_mut(&id) else {
                return Err("there is no provider with that id".to_string());
            };
            p.keys.push(ProviderKey {
                id: crate::config::new_id(),
                key: input.key,
                enabled: true,
                label: input.label,
            });
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::NOT_FOUND, Json(json!({"error": e}))))?;
    Ok(Json(json!({"ok": true})))
}

async fn api_delete_key(
    State(state): State<AppState>,
    Path((id, key_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .update(|config| {
            let Some(p) = config.provider_mut(&id) else {
                return Err("there is no provider with that id".to_string());
            };
            p.keys.retain(|k| k.id != key_id);
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::NOT_FOUND, Json(json!({"error": e}))))?;
    Ok(Json(json!({"ok": true})))
}

#[derive(serde::Deserialize)]
struct SetModels {
    models: Vec<String>,
}

async fn api_set_models(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let input: SetModels = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("the body is not json: {e}")})),
        )
    })?;
    state
        .update(|config| {
            let Some(p) = config.provider_mut(&id) else {
                return Err("there is no provider with that id".to_string());
            };
            p.models = input.models;
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::NOT_FOUND, Json(json!({"error": e}))))?;
    Ok(Json(json!({"ok": true})))
}

/// Fetch the vendor's own list of models.
async fn api_refresh_models(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let provider = {
        let config = state.config.read().await;
        config.provider(&id).cloned()
    };
    let Some(provider) = provider else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "there is no provider with that id"})),
        ));
    };
    let base = provider
        .base_url
        .clone()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({"error": "the provider has no url"}))))?;
    let url = client::models_url(&base);
    let key = provider
        .enabled_keys()
        .next()
        .map(|k| k.key.clone())
        .unwrap_or_default();
    let http = reqwest::Client::builder()
        .user_agent(format!("local-router/{}", client::VERSION))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    let mut req = http.get(&url);
    if !key.is_empty() {
        req = req.bearer_auth(&key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("the vendor answered {status}: {text}")})),
        ));
    }
    let value: Value = serde_json::from_str(&text).unwrap_or(json!({}));
    let mut models: Vec<String> = value
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get("id").and_then(Value::as_str))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    if models.is_empty() {
        models = value
            .get("models")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| {
                        i.get("name")
                            .or_else(|| i.get("id"))
                            .and_then(Value::as_str)
                    })
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
    }
    models.sort();
    models.dedup();
    Ok(Json(json!({"models": models})))
}

/// Send a tiny request to a provider, to see whether it answers.
async fn api_test_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let provider = {
        let config = state.config.read().await;
        config.provider(&id).cloned()
    };
    let Some(provider) = provider else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "there is no provider with that id"})),
        ));
    };
    let Some(key) = provider.enabled_keys().next().cloned() else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the provider has no key"})),
        ));
    };
    let Some(model) = provider.models.first().cloned() else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the provider exposes no model"})),
        ));
    };
    let base = provider
        .base_url
        .clone()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({"error": "the provider has no url"}))))?;
    let url = client::url_for(&base, provider.protocol, Some(&model), false);
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 8,
    });
    let headers = client::auth_headers(provider.protocol, &provider, &key.key, None);
    let http = reqwest::Client::builder()
        .user_agent(format!("local-router/{}", client::VERSION))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    let mut req = http.post(&url).json(&body);
    for (name, value) in &headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let started = std::time::Instant::now();
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({"ok": false, "error": e.to_string()})),
            ))
        }
    };
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let latency = started.elapsed().as_millis() as u64;
    let ok = (200..300).contains(&status);
    Ok(Json(json!({
        "ok": ok,
        "status": status,
        "latency_ms": latency,
        "model": model,
        "reply": text.chars().take(400).collect::<String>(),
    })))
}

async fn api_models(State(state): State<AppState>) -> Json<Value> {
    let config = state.config.read().await;
    let mut out = Vec::new();
    for p in &config.providers {
        for m in &p.models {
            out.push(json!({
                "id": format!("{}/{}", p.id, m),
                "provider": p.id,
                "provider_name": p.name,
                "model": m,
                "enabled": p.enabled,
            }));
        }
    }
    for g in &config.groups {
        out.push(json!({
            "id": format!("group/{}", g.id),
            "provider": "group",
            "provider_name": g.name,
            "model": g.id,
            "enabled": true,
            "group": true,
        }));
    }
    Json(json!({"models": out}))
}

// ----------------------------------------------------------------- groups --

async fn api_groups(State(state): State<AppState>) -> Json<Value> {
    let config = state.config.read().await;
    Json(json!({"groups": config.groups}))
}

#[derive(serde::Deserialize)]
struct GroupInput {
    id: Option<String>,
    name: String,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    routing: Option<Routing>,
    #[serde(default)]
    stays: Option<Stays>,
    #[serde(default)]
    listed: Option<bool>,
    #[serde(default)]
    note: Option<String>,
}

async fn api_add_group(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let input: GroupInput = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("the body is not json: {e}")})),
        )
    })?;
    let id = input
        .id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| slug(&input.name));
    let group = Group {
        id: id.clone(),
        name: input.name,
        models: input.models,
        routing: input.routing.unwrap_or_default(),
        weights: Default::default(),
        stays: input.stays.unwrap_or_default(),
        listed: input.listed.unwrap_or(true),
        note: input.note,
    };
    state
        .update(|config| {
            if config.groups.iter().any(|g| g.id == id) {
                return Err("there is already a group with that id".to_string());
            }
            config.groups.push(group);
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::CONFLICT, Json(json!({"error": e}))))?;
    Ok(Json(json!({"ok": true, "id": id})))
}

async fn api_update_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let input: GroupInput = serde_json::from_slice(&body).unwrap_or(GroupInput {
        id: None,
        name: String::new(),
        models: Vec::new(),
        routing: None,
        stays: None,
        listed: None,
        note: None,
    });
    state
        .update(|config| {
            let Some(g) = config.group_mut(&id) else {
                return Err("there is no group with that id".to_string());
            };
            if !input.name.is_empty() {
                g.name = input.name;
            }
            if !input.models.is_empty() {
                g.models = input.models;
            }
            if let Some(r) = input.routing {
                g.routing = r;
            }
            if let Some(s) = input.stays {
                g.stays = s;
            }
            if let Some(l) = input.listed {
                g.listed = l;
            }
            if input.note.is_some() {
                g.note = input.note;
            }
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::NOT_FOUND, Json(json!({"error": e}))))?;
    Ok(Json(json!({"ok": true})))
}

async fn api_delete_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .update(|config| {
            let before = config.groups.len();
            config.groups.retain(|g| g.id != id);
            if config.groups.len() == before {
                return Err("there is no group with that id".to_string());
            }
            for group in &mut config.groups {
                group
                    .models
                    .retain(|m| m != &format!("group/{id}"));
            }
            Ok(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::NOT_FOUND, Json(json!({"error": e}))))?;
    Ok(Json(json!({"ok": true})))
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "group".to_string()
    } else {
        out
    }
}

// --------------------------------------------------------------- settings --

async fn api_settings(State(state): State<AppState>) -> Json<Value> {
    let config = state.config.read().await;
    Json(json!({"settings": config.settings}))
}

async fn api_save_settings(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let input: Settings = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("the body is not json: {e}")})),
        )
    })?;
    state
        .update(|config| {
            config.settings = input;
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .ok();
    Ok(Json(json!({"ok": true})))
}

async fn api_caller_keys(State(state): State<AppState>) -> Json<Value> {
    let config = state.config.read().await;
    let keys: Vec<Value> = config
        .caller_keys
        .iter()
        .map(|k| {
            json!({
                "id": k.id,
                "name": k.name,
                "key": mask(&k.key),
                "enabled": k.enabled,
                "daily_tokens": k.daily_tokens,
                "monthly_cost": k.monthly_cost,
            })
        })
        .collect();
    Json(json!({"keys": keys}))
}

#[derive(serde::Deserialize)]
struct CallerKeyInput {
    name: String,
    key: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

async fn api_add_caller_key(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let input: CallerKeyInput = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("the body is not json: {e}")})),
        )
    })?;
    let key = input
        .key
        .unwrap_or_else(|| format!("sk-local-router-{}", uuid::Uuid::new_v4().simple()));
    state
        .update(|config| {
            config.caller_keys.push(crate::config::CallerKey {
                id: crate::config::new_id(),
                name: input.name,
                key,
                enabled: input.enabled.unwrap_or(true),
                daily_tokens: None,
                monthly_cost: None,
            });
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .ok();
    Ok(Json(json!({"ok": true})))
}

async fn api_delete_caller_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .update(|config| {
            config.caller_keys.retain(|k| k.id != id);
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?
        .ok();
    Ok(Json(json!({"ok": true})))
}

// ------------------------------------------------------------------ usage --

async fn api_usage(State(state): State<AppState>) -> Json<Value> {
    let records = usage::read_all(&crate::config::usage_file());
    let totals = usage::totals(&records);
    let by_model = usage::group_by(&records, |r| r.model.clone());
    let by_provider = usage::group_by(&records, |r| r.provider.clone());
    let by_day = usage::by_day(&records);
    let recent: Vec<Value> = records
        .iter()
        .rev()
        .take(200)
        .map(|r| serde_json::to_value(r).unwrap_or(json!({})))
        .collect();
    let _ = &state;
    Json(json!({
        "totals": totals,
        "by_model": by_model,
        "by_provider": by_provider,
        "by_day": by_day,
        "recent": recent,
    }))
}

/// A small helper the CLI and the page share.
pub fn provider_map(config: &crate::config::Config) -> BTreeMap<String, String> {
    config
        .providers
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect()
}
