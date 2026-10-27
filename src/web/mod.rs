//! The web page and the API behind it.
//!
//! The console trusts its own machine: on a loopback address every page and
//! API answers without a key. Bound anywhere else — a container, `--lan` —
//! the `/api` surface needs the same bearer key the gateway asks for, and
//! every write checks the Origin header so a web page in the user's browser
//! cannot drive the console cross-site.

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::config::{AppState, Group, Provider, ProviderKey, Routing, Settings, Stays};
use crate::gateway::client;
use crate::gateway::usage;
use crate::presets;

const INDEX_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/frontend/dist/index.html"
));
const APP_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/frontend/dist/assets/app.js"
));
const STYLE_CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/frontend/dist/assets/style.css"
));

/// Build the web server's routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/app.js", get(app_js))
        .route("/assets/style.css", get(style_css))
        .route("/api/status", get(api_status))
        .route("/api/workbuddy", get(api_workbuddy))
        .route("/api/presets", get(api_presets))
        .route("/api/providers", get(api_providers).post(api_add_provider))
        .route(
            "/api/providers/{id}",
            patch(api_update_provider).delete(api_delete_provider),
        )
        .route("/api/providers/{id}/keys", post(api_add_key))
        .route("/api/providers/{id}/keys/{key_id}", delete(api_delete_key))
        .route("/api/providers/{id}/models", post(api_set_models))
        .route("/api/providers/{id}/refresh", post(api_refresh_models))
        .route("/api/providers/{id}/test", post(api_test_provider))
        .route("/api/providers/{id}/oauth", post(api_oauth_start))
        .route("/api/providers/{id}/login", post(api_session_login_start))
        .route(
            "/api/providers/{id}/login/status",
            get(api_session_login_status),
        )
        .route("/api/providers/{id}/oauth", get(api_oauth_status))
        // The vendor redirects the browser here after the user signed in.
        .route("/oauth/callback", get(oauth_callback))
        .route("/api/models", get(api_models))
        .route("/api/groups", get(api_groups).post(api_add_group))
        .route(
            "/api/groups/{id}",
            patch(api_update_group).delete(api_delete_group),
        )
        .route("/api/settings", get(api_settings).put(api_save_settings))
        .route(
            "/api/caller-keys",
            get(api_caller_keys).post(api_add_caller_key),
        )
        .route("/api/caller-keys/{id}", delete(api_delete_caller_key))
        .route("/api/usage", get(api_usage))
        .route("/api/accounts", get(api_accounts))
        .route("/api/checkin", post(api_checkin))
        .route("/api/projects", get(api_projects).patch(api_update_project))
        .route("/api/projects/merge", post(api_merge_projects))
        // SPA deep links: everything that is not an API path or a known
        // asset falls back to the app shell.
        .fallback(spa_fallback)
        // Project ids are directories; a directory cannot ride in one path
        // segment (the %2F is decoded before routing), so an edit takes the
        // id as a query parameter instead.
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// The one guard every console request passes through.
async fn guard(State(state): State<AppState>, req: axum::extract::Request, next: Next) -> Response {
    // A cross-site write: browsers always send Origin on cross-site posts,
    // and a same-origin post's authority matches the host. Tools that talk
    // to the API directly send no Origin and pass untouched.
    if matches!(
        *req.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        if let Some(origin) = req
            .headers()
            .get(axum::http::header::ORIGIN)
            .and_then(|v| v.to_str().ok())
        {
            if !same_origin(origin, req.headers()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error": "cross-origin request refused"})),
                )
                    .into_response();
            }
        }
    }

    // Away from loopback the API needs a key — the same keys the gateway
    // accepts. The static page itself stays open; it holds no secrets.
    let remote = !listens_locally(&state).await;
    if remote && req.uri().path().starts_with("/api") {
        let presented = crate::gateway::presented_key(req.headers());
        let accepted = presented
            .filter(|k| {
                let keys = crate::gateway::allowed_keys(&state);
                !keys.is_empty() && keys.contains(k)
            })
            .is_some();
        if !accepted {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "send Authorization: Bearer <key>"})),
            )
                .into_response();
        }
    }

    next.run(req).await
}

/// True when an Origin header names the host the request was sent to.
fn same_origin(origin: &str, headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    authority.eq_ignore_ascii_case(host)
}

/// The console only trusts the machine when the page binds a loopback address.
async fn listens_locally(state: &AppState) -> bool {
    let addr = state.web_bind().await;
    if addr.eq_ignore_ascii_case("localhost") {
        return true;
    }
    addr.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

// ------------------------------------------------------------------ pages --

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// The SPA's routes live in the browser; any unknown path the user lands on
/// directly (a refresh on /providers, a bookmark, a shared link) must serve
/// the app and let the router pick the page.
///
/// API-shaped paths (`/api/…`) are excluded: a mistyped API call should get
/// a JSON 404 the console can display, not an HTML page it cannot parse.
async fn spa_fallback(uri: axum::http::Uri) -> Response {
    if uri.path().starts_with("/api/") {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": "no such api path"})),
        )
            .into_response();
    }
    Html(INDEX_HTML).into_response()
}

async fn app_js() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        APP_JS,
    )
}

async fn style_css() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
        STYLE_CSS,
    )
}

// ------------------------------------------------------------------- api --

async fn api_status(State(state): State<AppState>) -> Json<Value> {
    state.sync().await;
    let config = state.config.read().await;
    Json(json!({
        "version": client::VERSION,
        "providers": config.providers.len(),
        "groups": config.groups.len(),
        "models": config.model_ids().len(),
        "settings": config.settings,
    }))
}

async fn api_workbuddy() -> Json<Value> {
    // The WorkBuddy session lives on the machine that runs local-router;
    // a container without it simply reports that nobody is signed in.
    let mut out = json!({
        "signed_in": false,
        "account": Value::Null,
        "token_expires_at": Value::Null,
        "credits": Value::Null,
        "note": Value::Null,
    });
    match crate::workbuddy::read_session_file() {
        Ok(session) => {
            out["signed_in"] = json!(true);
            out["account"] = json!({
                "uid": session.account.uid,
                "nickname": session.account.nickname,
                "type": session.account.account_type,
            });
            out["token_expires_at"] = json!(session.expires_at);
            out["checkin"] = match crate::workbuddy::checkin_state().await {
                Some(state) => serde_json::to_value(state).unwrap_or(Value::Null),
                None => Value::Null,
            };
            match crate::workbuddy::credits().await {
                Ok(credits) => {
                    out["credits"] = serde_json::to_value(credits).unwrap_or(Value::Null)
                }
                Err(err) => out["note"] = json!(err.to_string()),
            }
        }
        Err(err) => out["note"] = json!(err.to_string()),
    }
    Json(out)
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
                    // A vendor that supports a browser login says so, and the
                    // console offers that instead of a key field:
                    //   oauth    — a standard OAuth 2.0 sign-in;
                    //   session  — the vendor's own login (WorkBuddy,
                    //              QwenWork), no key involved.
                    "oauth": p.oauth.is_some(),
                    "session_login": match p.auth {
                        Some(crate::config::AuthKind::WorkBuddy) => Some("workbuddy"),
                        Some(crate::config::AuthKind::QwenWork) => Some("qwenwork"),
                        _ => None,
                    },
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
    state.sync().await;
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
    #[serde(default)]
    headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    user_agent: Option<Option<String>>,
    #[serde(default)]
    forward_headers: Option<Vec<String>>,
    #[serde(default)]
    remove_headers: Option<Vec<String>>,
    #[serde(default)]
    header_order: Option<Vec<String>>,
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
        headers: None,
        user_agent: None,
        forward_headers: None,
        remove_headers: None,
        header_order: None,
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
            if let Some(v) = input.headers {
                p.headers = v;
            }
            if let Some(v) = input.user_agent {
                p.user_agent = v.filter(|s| !s.trim().is_empty());
            }
            if let Some(v) = input.forward_headers {
                p.forward_headers = v;
            }
            if let Some(v) = input.remove_headers {
                p.remove_headers = v;
            }
            if let Some(v) = input.header_order {
                p.header_order = v;
            }
            Ok(())
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
    // A session-backed provider lists its models from its own account.
    if provider.uses_session() {
        let models = crate::workbuddy::models().await.map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": e.to_string()})),
            )
        })?;
        let mut ids: Vec<String> = models.into_iter().collect();
        ids.sort();
        ids.dedup();
        return Ok(Json(json!({"models": ids})));
    }
    let base = provider.base_url.clone().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the provider has no url"})),
        )
    })?;
    let url = client::models_url(&base);
    let key = provider
        .enabled_keys()
        .next()
        .map(|k| k.key.clone())
        .unwrap_or_default();
    let http = reqwest::Client::builder()
        .user_agent(format!("local-router/{}", client::VERSION))
        .build()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    let mut req = http.get(&url);
    if !key.is_empty() {
        req = req.bearer_auth(&key);
    }
    let resp = req.send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": e.to_string()})),
        )
    })?;
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
    // Auto-include: glob matches the vendor just listed are folded into the
    // provider's model list on disk, so a new model shows up without a
    // release or a manual edit.
    let mut auto_added: Vec<String> = Vec::new();
    if !provider.auto_include.is_empty() {
        for m in &models {
            if !provider.models.contains(m)
                && provider
                    .auto_include
                    .iter()
                    .any(|pat| auto_include_match(pat, m))
            {
                auto_added.push(m.clone());
            }
        }
    }
    if !auto_added.is_empty() {
        let to_add = auto_added.clone();
        let _ = state
            .update(move |config| {
                if let Some(p) = config.provider_mut(&id) {
                    for m in to_add {
                        if !p.models.contains(&m) {
                            p.models.push(m);
                        }
                    }
                    p.models.sort();
                    Ok::<(), String>(())
                } else {
                    Err(format!("the provider {id} vanished mid-refresh"))
                }
            })
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;
    }
    let merged = if auto_added.is_empty() {
        models.clone()
    } else {
        let mut all = models.clone();
        all.extend(auto_added.iter().cloned());
        all.sort();
        all.dedup();
        all
    };
    Ok(Json(json!({
        "models": merged,
        "auto_included": auto_added,
    })))
}
pub(crate) fn auto_include_match(pattern: &str, model: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == model;
    }
    // Fold every `*` into two: head must prefix, tail must suffix, and the
    // middle only has to exist (multiple stars act as one greedy star).
    let mut split = pattern.split('*');
    let head = split.next().unwrap_or_default();
    let last = split.next_back().unwrap_or_default();
    model.len() >= head.len() + last.len() && model.starts_with(head) && model.ends_with(last)
}

#[cfg(test)]
mod auto_include_tests {
    use super::auto_include_match as m;

    #[test]
    fn globs_match_prefix_suffix_and_exact() {
        assert!(m("mock-*", "mock-small"));
        assert!(m("mock-*", "mock-"));
        assert!(!m("mock-*", "not-mock-small"));
        assert!(m("*-turbo", "glm-5-turbo"));
        assert!(!m("*-turbo", "glm-5-turbo-v"));
        assert!(m("glm-4.6", "glm-4.6"));
        assert!(!m("glm-4.6", "glm-4.5"));
        assert!(m("*", "anything"));
        assert!(m("*mini*", "gpt-4o-mini-2024"));
    }
}

/// Send a tiny request to a provider, to see whether it answers.
/// Start a browser sign-in for a provider that wants one.
///
/// The console calls this, gets the authorize URL, and sends the browser
/// there. The vendor redirects back to `/oauth/callback`, which finishes
/// the flow; the console polls the status endpoint.
async fn api_oauth_start(
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
    let Some(oauth) = provider.oauth.clone() else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "this provider has no browser sign-in configured"})),
        ));
    };
    let redirect = format!("{}/oauth/callback", console_base(&state).await);
    match crate::oauth::start_flow(&id, &oauth, &redirect) {
        Ok((url, state_token)) => Ok(Json(json!({"url": url, "state": state_token}))),
        Err(err) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": err.to_string()})),
        )),
    }
}

/// Start a browser sign-in for a vendor that keeps its own session
/// (WorkBuddy, QwenWork): no key, no OAuth — the vendor's own login page.
///
/// The console opens the returned URL and polls the status endpoint; the
/// session lands in local-router's own 0600 file when the sign-in lands.
async fn api_session_login_start(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // The provider may not exist yet: the console offers the sign-in on the
    // preset, before anything is configured. Create it on sight — a
    // session-backed vendor needs no key, so an empty provider is complete.
    if state.config.read().await.provider(&id).is_none() {
        let Some(preset) = crate::presets::get(&id) else {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("there is no provider called {id}")})),
            ));
        };
        let provider = crate::presets::to_provider(preset, None);
        let _ = state
            .update(|config| {
                config.providers.push(provider.clone());
                Ok::<(), String>(())
            })
            .await;
    }
    let kind = {
        let config = state.config.read().await;
        config.provider(&id).and_then(|p| p.auth)
    };
    match kind {
        Some(crate::config::AuthKind::WorkBuddy) => {
            let session = crate::workbuddy::start_login().await.map_err(oauth_err)?;
            crate::session_logins::remember(
                &id,
                crate::session_logins::Pending::WorkBuddy {
                    state: session.state.clone(),
                },
            );
            Ok(Json(
                json!({"url": session.auth_url, "state": session.state}),
            ))
        }
        Some(crate::config::AuthKind::QwenWork) => {
            let session = crate::qwen::login::start_login().await.map_err(oauth_err)?;
            crate::session_logins::remember(
                &id,
                crate::session_logins::Pending::QwenWork {
                    state: session.state.clone(),
                },
            );
            Ok(Json(json!({"url": session.auth_url})))
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "this vendor does not sign in with a browser session"})),
        )),
    }
}

/// Whether the browser sign-in finished, and what it produced.
async fn api_session_login_status(
    State(app_state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let pending = crate::session_logins::take_or_peek(&id);
    let Some(pending) = pending else {
        // Nothing in flight: the session file tells us whether it worked.
        let signed_in = match crate::config::AuthKind::WorkBuddy {
            crate::config::AuthKind::WorkBuddy => crate::workbuddy::read_session_file().is_ok(),
            crate::config::AuthKind::QwenWork => crate::qwen::login::read_session_file().is_ok(),
            crate::config::AuthKind::Stored => false,
        };
        return Ok(Json(json!({"done": signed_in, "waiting": false})));
    };
    match pending {
        crate::session_logins::Pending::WorkBuddy { state } => {
            match crate::workbuddy::poll_login(&state).await {
                Ok(crate::workbuddy::LoginPoll::Waiting) => {
                    Ok(Json(json!({"done": false, "waiting": true})))
                }
                Ok(crate::workbuddy::LoginPoll::Ready(result)) => {
                    let nickname = result
                        .account
                        .get("nickname")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let _ = crate::workbuddy::store_session(
                        &result.session,
                        &result.account,
                        "default",
                    )
                    .await;
                    crate::session_logins::forget(&id);
                    // Make sure the provider exists and is wired.
                    wire_session_provider(&app_state, "workbuddy").await;
                    Ok(Json(
                        json!({"done": true, "waiting": false, "account": nickname}),
                    ))
                }
                Err(err) => Err(oauth_err(err)),
            }
        }
        crate::session_logins::Pending::QwenWork { state: login_state } => {
            match crate::qwen::login::poll_login(&login_state).await {
                Ok(crate::qwen::login::LoginPoll::Waiting) => {
                    Ok(Json(json!({"done": false, "waiting": true})))
                }
                Ok(crate::qwen::login::LoginPoll::Ready(token)) => {
                    let account = crate::qwen::login::login_account(&login_state, &token)
                        .await
                        .unwrap_or(serde_json::Value::Null);
                    let nickname = account
                        .get("nickname")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let _ = crate::qwen::login::store_session(
                        &token,
                        &account,
                        &login_state.machine_id,
                    )
                    .await;
                    crate::session_logins::forget(&id);
                    wire_session_provider(&app_state, "qwenwork").await;
                    Ok(Json(
                        json!({"done": true, "waiting": false, "account": nickname}),
                    ))
                }
                Err(err) => Err(oauth_err(err)),
            }
        }
    }
}

/// Add the provider a session-backed vendor needs, if it is not there yet.
async fn wire_session_provider(state: &AppState, kind: &str) {
    let preset_id = match kind {
        "workbuddy" => "workbuddy",
        "qwenwork" => "qwenwork",
        _ => return,
    };
    let has = {
        let config = state.config.read().await;
        config.provider(preset_id).is_some()
    };
    if has {
        return;
    }
    let Some(preset) = crate::presets::get(preset_id) else {
        return;
    };
    let provider = crate::presets::to_provider(preset, None);
    let _ = state
        .update(|config| {
            config.providers.push(provider.clone());
            Ok::<(), String>(())
        })
        .await;
}

/// Turn a login error into a JSON answer the console can show.
fn oauth_err(err: anyhow::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error": err.to_string()})),
    )
}

/// Claim a WorkBuddy account's daily check-in from the console.
async fn api_checkin(
    State(_state): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let _ = body;
    // The console asks for the workbuddy account by provider id; the
    // default session is the one signed in through the UI.
    let outcome = crate::workbuddy::claim_checkin().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": err.to_string()})),
        )
    })?;
    Ok(Json(serde_json::to_value(&outcome).unwrap_or(Value::Null)))
}

/// Every signed-in account, for the Accounts page: who is signed in, what
/// the plan is, what is left of it, and which provider serves it.
async fn api_accounts(State(state): State<AppState>) -> Json<Value> {
    let config = state.config.read().await;
    let session_providers: Vec<_> = config
        .providers
        .iter()
        .filter(|p| p.enabled && p.uses_session())
        .cloned()
        .collect();
    drop(config);

    let mut accounts: Vec<Value> = Vec::new();
    for provider in &session_providers {
        match provider.auth {
            Some(crate::config::AuthKind::WorkBuddy) => {
                let mut entry = json!({
                    "provider": provider.id,
                    "vendor": "workbuddy",
                    "kind": "workbuddy",
                });
                match crate::workbuddy::read_session_file() {
                    Ok(session) => {
                        entry["signed_in"] = json!(true);
                        entry["account"] = json!({
                            "nickname": session.account.nickname,
                            "uid": session.account.uid,
                            "type": session.account.account_type,
                        });
                        entry["token_expires_at"] = json!(session.expires_at);
                        match crate::workbuddy::credits().await {
                            Ok(credits) => {
                                entry["credits"] =
                                    serde_json::to_value(&credits).unwrap_or(Value::Null);
                            }
                            Err(err) => entry["error"] = json!(err.to_string()),
                        }
                    }
                    Err(err) => {
                        entry["signed_in"] = json!(false);
                        entry["error"] = json!(err.to_string());
                    }
                }
                accounts.push(entry);
            }
            Some(crate::config::AuthKind::QwenWork) => {
                let mut entry = json!({
                    "provider": provider.id,
                    "vendor": "qwenwork",
                    "kind": "qwenwork",
                });
                match crate::qwen::login::read_session_file() {
                    Ok(session) => {
                        entry["signed_in"] = json!(true);
                        entry["account"] = json!({
                            "nickname": session.account.nickname,
                            "uid": session.account.uid,
                            "email": session.account.email,
                        });
                        entry["token_expires_at"] = json!(session.expires_at);
                        match crate::qwen::login::session().await {
                            Ok(live) => match crate::qwen::client::account_quota(&live).await {
                                Ok(quota) => {
                                    entry["plan"] = json!(quota.plan);
                                    entry["balance"] = json!(quota.balance);
                                    entry["pools"] = json!(quota.pools);
                                    entry["resets_at"] = json!(quota.resets_at);
                                }
                                Err(err) => entry["error"] = json!(err.to_string()),
                            },
                            Err(err) => entry["error"] = json!(err.to_string()),
                        }
                    }
                    Err(err) => {
                        entry["signed_in"] = json!(false);
                        entry["error"] = json!(err.to_string());
                    }
                }
                accounts.push(entry);
            }
            _ => {}
        }
    }
    Json(json!({ "accounts": accounts }))
}

/// Whether the sign-in finished, and what it produced.
async fn api_oauth_status(
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
    // The stored credential means the sign-in went through.
    let signed_in = provider
        .keys
        .iter()
        .any(|k| k.key.starts_with("oauth:") || !k.key.trim().is_empty());
    Ok(Json(json!({
        "signed_in": signed_in,
        "keys": provider.keys.len(),
    })))
}

/// Where the vendor sends the browser back to.
async fn console_base(state: &AppState) -> String {
    let settings = state.config.read().await.settings.clone();
    format!("http://{}:{}", settings.gateway_addr, settings.gateway_port)
}

/// The vendor redirects the browser here with the code. We swap it for a
/// credential, store it on the provider, and send the user back to the
/// console with a note.
async fn oauth_callback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<BTreeMap<String, String>>,
) -> Response {
    let code = params.get("code").or_else(|| params.get("auth_code"));
    let state_token = params.get("state").cloned();
    let error = params.get("error").cloned();

    let title = "local-router";
    let (heading, detail, ok) = match (error, code, state_token) {
        (Some(err), _, _) => ("Sign-in failed", format!("the vendor said: {err}"), false),
        (None, Some(code), Some(state_token)) => {
            match crate::oauth::finish_flow(code, &state_token).await {
                Ok((provider_id, token)) => {
                    // Store the credential on the provider and save it.
                    let key = token.api_key.clone();
                    let stored = state
                        .update(|config| {
                            let Some(provider) = config.provider_mut(&provider_id) else {
                                return Err(format!("there is no provider called {provider_id}"));
                            };
                            provider.keys.push(ProviderKey::new(key.clone()));
                            Ok::<(), String>(())
                        })
                        .await;
                    match stored {
                        Ok(Ok(())) => (
                            "Signed in",
                            format!(
                                "the credential for {provider_id} is stored — you can close this tab"
                            ),
                            true,
                        ),
                        Ok(Err(err)) => ("Sign-in failed", err, false),
                        Err(err) => ("Sign-in failed", err.to_string(), false),
                    }
                }
                Err(err) => ("Sign-in failed", err.to_string(), false),
            }
        }
        _ => (
            "Sign-in failed",
            "the vendor sent back neither a code nor a state".to_string(),
            false,
        ),
    };

    let color = if ok { "#9ef0bf" } else { "#ff8d8d" };
    let html = format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>{title}</title><style>body{{background:#0a0d0e;color:#edf2f0;font:14px -apple-system,BlinkMacSystemFont,sans-serif;display:grid;place-content:center;min-height:100dvh;text-align:center;gap:8px}}h1{{font-size:20px;letter-spacing:-0.03em;margin:0}}p{{color:#7d8988;font-size:12px;max-width:34ch;margin:0}}a{{color:{color};font-size:12px}}</style></head><body><h1>{heading}</h1><p>{detail}</p><p><a href="/providers">← back to the console</a></p></body></html>"#
    );
    Html(html).into_response()
}

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
    let Some(model) = provider.models.first().cloned() else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the provider exposes no model"})),
        ));
    };
    // A session-backed provider has no stored key: the session is read
    // when the request goes out.
    let headers = if provider.uses_session() {
        let session = crate::workbuddy::session().await.map_err(|e| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": e.to_string()})),
            )
        })?;
        client::session_headers(provider.protocol, &provider, &session)
    } else {
        let Some(key) = provider.enabled_keys().next().cloned() else {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "the provider has no key"})),
            ));
        };
        client::auth_headers(provider.protocol, &provider, &key.key, None, None)
    };
    let base = provider.base_url.clone().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the provider has no url"})),
        )
    })?;
    let url = match &provider.chat_path {
        Some(path) => format!("{}{}", base.trim_end_matches('/'), path),
        None => client::url_for(&base, provider.protocol, Some(&model), false),
    };
    let mut body = json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 8,
    });
    if provider.stream_only {
        body["stream"] = json!(true);
        body["stream_options"] = json!({"include_usage": true});
    }
    let http = reqwest::Client::builder()
        .user_agent(format!("local-router/{}", client::VERSION))
        .build()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
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
    state.sync().await;
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

async fn api_groups(State(state): State<AppState>) -> Json<Value> {
    state.sync().await;
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
    /// The weight of each member, for `weight` routing (member=weight).
    #[serde(default)]
    weights: Option<BTreeMap<String, u32>>,
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
        weights: None,
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
            if let Some(w) = input.weights {
                g.weights = w;
            }
            Ok(())
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
                group.models.retain(|m| m != &format!("group/{id}"));
            }
            Ok(())
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
    state.sync().await;
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
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
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .ok();
    Ok(Json(json!({"ok": true})))
}

// ------------------------------------------------------------------ usage --

/// How far back to look, and in what money.
type UsageWindow = (i64, String);

/// The shape the Usage page asks for: how far back, and in what money.
fn usage_query(query: &BTreeMap<String, String>) -> UsageWindow {
    let period = query.get("period").map(|s| s.as_str()).unwrap_or("7d");
    let days: i64 = match period {
        "today" | "24h" => 1,
        "30d" => 30,
        "all" => 36500,
        _ => 7,
    };
    let currency = query
        .get("currency")
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "usd".to_string());
    (days, currency)
}

async fn api_usage(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
) -> Json<Value> {
    let (days, currency) = usage_query(&query);
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();

    // One owned copy of the records, newest last, from either store.
    let use_sqlite = state.sqlite.is_some();
    let records: Vec<usage::UsageRecord> = if let Some(store) = &state.sqlite {
        store.all_since(&cutoff)
    } else {
        usage::read_all(&state.usage_path())
            .into_iter()
            .filter(|r| r.time.as_str() >= cutoff.as_str())
            .collect()
    };
    // Reuse the legacy `recent` order (newest first) for the request log.
    let recent: Vec<Value> = records
        .iter()
        .rev()
        .take(200)
        .map(|r| serde_json::to_value(r).unwrap_or(json!({})))
        .collect();

    // Aggregate every dimension from the same records, so the page never
    // shows a field the backend forgot to fill.
    let totals = usage::totals(&records);
    let by_model = usage::group_by(&records, |r| r.model.clone());
    let by_provider = usage::group_by(&records, |r| r.provider.clone());
    let _ = &by_provider;
    let by_day = usage::by_day(&records);
    let by_session = usage::group_by(&records, |r| r.session_id.clone().unwrap_or_default());
    let by_agent = usage::group_by(&records, |r| {
        r.agent.clone().unwrap_or_else(|| "unknown".into())
    });
    let by_project = usage::group_by(&records, |r| r.project.clone().unwrap_or_default());
    let by_key = usage::group_by(&records, |r| {
        r.key_label
            .clone()
            .or_else(|| r.key_id.clone())
            .unwrap_or_else(|| "default".into())
    });

    // Per-hour request counts for the Requests timeline (line/bar toggle).
    let mut hour_map: BTreeMap<String, u64> = BTreeMap::new();
    for r in &records {
        let hour = r.time.chars().take(13).collect::<String>();
        *hour_map.entry(hour).or_insert(0) += 1;
    }
    let requests_timeline: Vec<Value> = hour_map
        .into_iter()
        .map(|(hour, count)| json!({ "hour": hour, "requests": count }))
        .collect();

    // Per-provider cost, as a map (the legacy by_provider_costs field, now
    // actually returned so the platform-cost panels render).
    let by_provider_costs: BTreeMap<String, usage::Totals> = by_provider.iter().cloned().collect();

    // Context: average fill ratio and score per provider, over the requests
    // that carried a breakdown. Values: (count, sum_fill, system, tool, file,
    // conversation, cache_read).
    type CtxAgg = (u64, f64, u64, u64, u64, u64, u64);
    let mut ctx_providers: BTreeMap<String, CtxAgg> = BTreeMap::new();
    let mut ctx_total = (0u64, 0u64, 0u64, 0u64);
    for r in &records {
        if let Some(ctx) = &r.context {
            ctx_total.0 += ctx.system_tokens;
            ctx_total.1 += ctx.tool_tokens;
            ctx_total.2 += ctx.file_tokens;
            ctx_total.3 += ctx.conversation_tokens;
            let entry = ctx_providers.entry(r.provider.clone()).or_default();
            entry.0 += 1;
            entry.1 += ctx.fill_ratio();
            entry.2 += ctx.system_tokens;
            entry.3 += ctx.tool_tokens;
            entry.4 += ctx.file_tokens;
            entry.5 += ctx.conversation_tokens;
            entry.6 += ctx.cache_read_tokens;
        }
    }
    let context = json!({
        "by_provider": ctx_providers
            .into_iter()
            .map(|(provider, e)| json!({
                "provider": provider,
                "requests": e.0,
                "avg_fill": if e.0 > 0 { e.1 / e.0 as f64 } else { 0.0 },
                "system_tokens": e.2,
                "tool_tokens": e.3,
                "file_tokens": e.4,
                "conversation_tokens": e.5,
                "cache_read_tokens": e.6,
            }))
            .collect::<Vec<_>>(),
        "totals": {
            "system_tokens": ctx_total.0,
            "tool_tokens": ctx_total.1,
            "file_tokens": ctx_total.2,
            "conversation_tokens": ctx_total.3,
        },
        "captured": records.iter().any(|r| r.context.is_some()),
    });

    // Sessions: title each session id from the session book.
    let sessions = crate::sessions::load();
    let by_session_json: Vec<Value> = by_session
        .into_iter()
        .filter(|(id, _)| !id.is_empty())
        .map(|(id, t)| {
            let session = sessions.sessions.get(&id);
            json!({
                "id": id,
                "title": session.map(|s| s.title.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| id.clone()),
                "requests": t.requests,
                "input_tokens": t.input_tokens,
                "output_tokens": t.output_tokens,
                "cost_usd": t.cost_usd,
                "credits": t.credits,
                "failed": t.failed,
                "last_seen": session.map(|s| s.last_seen.clone()).unwrap_or_default(),
            })
        })
        .collect();

    let totals_json = json!({
        "requests": totals.requests,
        "failed": totals.failed,
        "input_tokens": totals.input_tokens,
        "output_tokens": totals.output_tokens,
        "cache_read_tokens": totals.cache_read_tokens,
        "cache_write_tokens": totals.cache_write_tokens,
        "reasoning_tokens": totals.reasoning_tokens,
        "cost_usd": totals.cost_usd,
        "credits": totals.credits,
        "unpriced": totals.unpriced,
    });
    let map_to_array = |rows: Vec<(String, usage::Totals)>| {
        rows.into_iter()
            .map(|(label, t)| {
                json!([
                    label,
                    {
                        "requests": t.requests,
                        "failed": t.failed,
                        "input_tokens": t.input_tokens,
                        "output_tokens": t.output_tokens,
                        "cache_read_tokens": t.cache_read_tokens,
                        "cache_write_tokens": t.cache_write_tokens,
                        "reasoning_tokens": t.reasoning_tokens,
                        "cost_usd": t.cost_usd,
                        "credits": t.credits,
                        "unpriced": t.unpriced,
                    }
                ])
            })
            .collect::<Vec<_>>()
    };
    let by_model_json = map_to_array(by_model);
    let by_provider_json = map_to_array(by_provider);
    let by_agent_json = map_to_array(by_agent);
    let by_key_json = map_to_array(by_key);
    let by_project_json = map_to_array(by_project);
    let by_day_json = by_day
        .into_iter()
        .map(|(day, t)| {
            json!([
                day,
                {
                    "requests": t.requests,
                    "failed": t.failed,
                    "input_tokens": t.input_tokens,
                    "output_tokens": t.output_tokens,
                    "cost_usd": t.cost_usd,
                    "credits": t.credits,
                }
            ])
        })
        .collect::<Vec<_>>();

    Json(json!({
        "storage": if use_sqlite { "sqlite" } else { "jsonl" },
        "currency": currency,
        "totals": totals_json,
        "by_model": by_model_json,
        "by_provider": by_provider_json,
        "by_provider_costs": by_provider_costs,
        "by_agent": by_agent_json,
        "by_key": by_key_json,
        "by_project": by_project_json,
        "by_day": by_day_json,
        "by_session": by_session_json,
        "requests_timeline": requests_timeline,
        "context": context,
        "quota": quota_summary(&state).await,
        "recent": recent,
    }))
}

/// The quota summary for the Usage page's Quota panel. Reads the WorkBuddy
/// account when it is configured; every other provider reports no quota (it
/// bills by token, not by window) and is listed as unlimited.
async fn quota_summary(state: &AppState) -> Value {
    let config = state.config.read().await;
    let mut accounts: Vec<Value> = Vec::new();
    for provider in &config.providers {
        if !provider.uses_session() {
            accounts.push(json!({
                "provider": provider.id,
                "name": provider.name,
                "kind": "token",
                "windows": [],
                "unlimited": true,
            }));
            continue;
        }
        if provider.id == "qwenwork" {
            match crate::qwen::login::session().await {
                Ok(session) => match crate::qwen::client::account_quota(&session).await {
                    Ok(q) => accounts.push(json!({
                        "provider": "qwenwork",
                        "name": "QwenWork（千问办公）",
                        "kind": "subscription",
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
                        "unlimited": false,
                        "read_at": chrono::Utc::now().to_rfc3339(),
                    })),
                    Err(err) => accounts.push(json!({
                        "provider": "qwenwork",
                        "name": "QwenWork（千问办公）",
                        "kind": "token",
                        "windows": [],
                        "unlimited": true,
                        "error": err.to_string(),
                    })),
                },
                Err(err) => accounts.push(json!({
                    "provider": "qwenwork",
                    "name": "QwenWork（千问办公）",
                    "kind": "token",
                    "windows": [],
                    "unlimited": true,
                    "error": err.to_string(),
                })),
            }
            continue;
        }
        match crate::workbuddy::quota().await {
            Ok(q) => accounts.push(json!({
                "provider": q.provider,
                "name": q.name,
                "kind": q.kind,
                "user": q.user,
                "windows": q.windows
                    .iter()
                    .map(|w| json!({
                        "name": w.name,
                        "used": w.used,
                        "remaining": w.remaining,
                        "resets_at": w.resets_at,
                        "display": w.display,
                        "unit": w.unit,
                        "unlimited": w.unlimited(),
                        "holds_why": w.holds_why,
                    }))
                    .collect::<Vec<_>>(),
                "unlimited": false,
                "read_at": q.read_at,
            })),
            Err(err) => accounts.push(json!({
                "provider": provider.id,
                "name": provider.name,
                "kind": "token",
                "windows": [],
                "unlimited": true,
                "error": err.to_string(),
            })),
        }
    }
    json!(accounts)
}

// ---------------------------------------------------------------- projects --

/// One project row on the Usage page: the registry entry joined with what
/// the period's requests charged it.
#[derive(serde::Serialize)]
struct ProjectRow {
    id: String,
    name: String,
    color: String,
    created_at: String,
    budget_tokens: Option<u64>,
    budget_cost: Option<f64>,
    requests: u64,
    failed: u64,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    credits: f64,
    last_seen: String,
}

async fn project_rows(state: &AppState, days: i64) -> Vec<ProjectRow> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    let records: Vec<usage::UsageRecord> = if let Some(store) = &state.sqlite {
        store.all_since(&cutoff)
    } else {
        usage::read_all(&state.usage_path())
            .into_iter()
            .filter(|r| r.time.as_str() >= cutoff.as_str())
            .collect()
    };
    type ProjectUsage = (u64, u64, u64, u64, f64, f64, String);
    let mut usage_by_project: BTreeMap<String, ProjectUsage> = BTreeMap::new();
    for r in &records {
        let Some(project) = &r.project else { continue };
        let entry = usage_by_project
            .entry(project.clone())
            .or_insert_with(|| (0, 0, 0, 0, 0.0, 0.0, String::new()));
        entry.0 += 1;
        if !r.ok {
            entry.1 += 1;
            continue;
        }
        entry.2 += r.input_tokens;
        entry.3 += r.output_tokens;
        if let Some(usd) = r.cost_usd {
            entry.4 += usd;
        }
        if let Some(c) = r.credits {
            entry.5 += c;
        }
        if r.time.as_str() > entry.6.as_str() {
            entry.6 = r.time.clone();
        }
    }
    let registry = crate::projects::load();
    let mut rows: Vec<ProjectRow> = Vec::new();
    for (id, project) in &registry.projects {
        let (requests, failed, input_tokens, output_tokens, cost_usd, credits, last_seen) =
            usage_by_project.remove(id).unwrap_or_default();
        rows.push(ProjectRow {
            id: id.clone(),
            name: project.display_name().to_string(),
            color: project.color.clone(),
            created_at: project.created_at.clone(),
            budget_tokens: project.budget_tokens,
            budget_cost: project.budget_cost,
            requests,
            failed,
            input_tokens,
            output_tokens,
            cost_usd,
            credits,
            last_seen,
        });
    }
    // A directory usage knows but the registry lost (a cleared projects.json)
    // still gets a row: the data is honest even when the registry is behind.
    for (id, (requests, failed, input_tokens, output_tokens, cost_usd, credits, last_seen)) in
        usage_by_project
    {
        rows.push(ProjectRow {
            id: id.clone(),
            name: id.clone(),
            color: "#9ef0bf".to_string(),
            created_at: String::new(),
            budget_tokens: None,
            budget_cost: None,
            requests,
            failed,
            input_tokens,
            output_tokens,
            cost_usd,
            credits,
            last_seen,
        });
    }
    rows.sort_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then(b.requests.cmp(&a.requests))
    });
    rows
}

/// The project registry with the period's spend, for the Usage page.
async fn api_projects(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
) -> Json<Value> {
    let (days, _) = usage_query(&query);
    let rows = project_rows(&state, days).await;
    Json(json!({ "projects": rows }))
}

/// Rename a project, recolor it, or set or clear its budgets.
async fn api_update_project(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
    axum::Json(patch): axum::Json<crate::projects::ProjectPatch>,
) -> Response {
    let Some(id) = query.get("id").cloned() else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "id is required"})),
        )
            .into_response();
    };
    let id = urlencoding_decode(&id);
    let ok = crate::projects::patch(&id, &patch);
    if !ok {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": "no such project"})),
        )
            .into_response();
    }
    let (days, _) = usage_query(&query);
    let rows = project_rows(&state, days).await;
    Json(json!({ "ok": true, "projects": rows })).into_response()
}

/// Merge one project into another: the records are re-keyed in both stores.
async fn api_merge_projects(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    let Some(from) = body.get("from").and_then(Value::as_str) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "from is required"})),
        )
            .into_response();
    };
    let Some(into) = body.get("into").and_then(Value::as_str) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "into is required"})),
        )
            .into_response();
    };
    if !crate::projects::merge(from, into) {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": "merge failed: target unknown or same"})),
        )
            .into_response();
    }
    // Re-key the stored records so the history follows the merge.
    merge_project_records(&state, from, into);
    let (days, _) = usage_query(&query);
    let rows = project_rows(&state, days).await;
    Json(json!({ "ok": true, "projects": rows })).into_response()
}

/// Re-key every stored usage record from one project to another.
fn merge_project_records(state: &AppState, from: &str, into: &str) {
    let path = state.usage_path();
    let mut lines = Vec::new();
    {
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        for line in text.lines() {
            if let Ok(mut rec) = serde_json::from_str::<usage::UsageRecord>(line) {
                if rec.project.as_deref() == Some(from) {
                    rec.project = Some(into.to_string());
                }
                if let Ok(json) = serde_json::to_string(&rec) {
                    lines.push(json);
                }
            } else {
                lines.push(line.to_string());
            }
        }
    }
    if lines.is_empty() {
        return;
    }
    let _ = std::fs::write(&path, lines.join("\n") + "\n");
    if let Some(store) = &state.sqlite {
        store.rekey_project(from, into);
    }
}

/// Decode a percent-encoded path segment back into a directory. Project ids
/// are directories, so `/api/projects/~%2Fwork%2Facme` names the same project
/// the agent reported. A segment without escapes passes through untouched.
fn urlencoding_decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A small helper the CLI and the page share.
pub fn provider_map(config: &crate::config::Config) -> BTreeMap<String, String> {
    config
        .providers
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn headers_with_host(host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_str(host).unwrap(),
        );
        headers
    }

    #[test]
    fn an_origin_matching_the_host_is_same_origin() {
        let headers = headers_with_host("127.0.0.1:3426");
        assert!(same_origin("http://127.0.0.1:3426", &headers));
        // The scheme is not compared: a page served over http/https to the
        // same host is the same site as far as this check goes.
        assert!(same_origin("https://127.0.0.1:3426", &headers));
        // Host names are case-insensitive.
        assert!(same_origin(
            "http://LOCALHOST:3426",
            &headers_with_host("localhost:3426")
        ));
        // An origin without a scheme still matches (defensive).
        assert!(same_origin("127.0.0.1:3426", &headers));
    }

    #[test]
    fn a_foreign_origin_is_refused() {
        let headers = headers_with_host("127.0.0.1:3426");
        assert!(!same_origin("http://evil.example.com", &headers));
        assert!(!same_origin("http://127.0.0.1:9999", &headers));
        assert!(!same_origin("http://127.0.0.1.evil.com", &headers));
    }

    #[test]
    fn a_missing_host_header_refuses_everything() {
        let headers = HeaderMap::new();
        assert!(!same_origin("http://127.0.0.1:3426", &headers));
    }
}
