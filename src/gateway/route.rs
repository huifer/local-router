//! Deciding who answers a request.
//!
//! A request names a model: either `provider/model` or `group/id`. The
//! router turns that name into an ordered list of *targets* — a provider, a
//! key and a model — that the gateway tries in turn until one answers.

use std::collections::{BTreeMap, HashMap, HashSet};

use tokio::sync::Mutex;

use crate::config::{Config, Group, Provider, Routing};
use crate::protocol::ir::ApiError;

use super::client::Target;

/// A plan for one request: the targets to try, and why.
#[derive(Clone)]
pub struct RoutePlan {
    pub targets: Vec<Target>,
    /// The group the request went through, when it went through one.
    pub group_id: Option<String>,
    /// The group's members, in the order they will be tried.
    pub members: Vec<String>,
    /// The routing mode that produced this order.
    pub routing: Option<Routing>,
    /// A human sentence saying what happened, for the routing view.
    pub reason: String,
}

/// What the router remembers between requests.
#[derive(Default)]
struct RouterState {
    /// How many times each model was asked for.
    usage: HashMap<String, u64>,
    /// The cursor each group's rotation is at.
    rotate: HashMap<String, u64>,
    /// Which target answered which session.
    affinity: HashMap<String, String>,
}

/// The router's memory.
#[derive(Default)]
pub struct Router {
    state: Mutex<RouterState>,
}

impl Router {
    pub fn new() -> Self {
        Router::default()
    }

    pub async fn note_used(&self, model: &str) {
        let mut state = self.state.lock().await;
        *state.usage.entry(model.to_string()).or_insert(0) += 1;
    }

    pub async fn usage_of(&self, model: &str) -> u64 {
        self.state
            .lock()
            .await
            .usage
            .get(model)
            .copied()
            .unwrap_or(0)
    }

    pub async fn note_affinity(&self, session: &str, target_key: &str) {
        let mut state = self.state.lock().await;
        state
            .affinity
            .insert(session.to_string(), target_key.to_string());
    }

    pub async fn affinity_of(&self, session: &str) -> Option<String> {
        self.state.lock().await.affinity.get(session).cloned()
    }

    /// Work out who should answer `requested`.
    pub async fn plan(
        &self,
        config: &Config,
        requested: &str,
        session: Option<&str>,
    ) -> Result<RoutePlan, ApiError> {
        let requested = requested.trim();
        if let Some(group_id) = requested.strip_prefix("group/") {
            let group = config.group(group_id).ok_or_else(|| {
                ApiError::NotFound(format!("there is no group called {group_id}"))
            })?;
            return self.plan_group(config, group, session).await;
        }

        let (provider_id, model) = Config::split_model(requested).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "{requested} is not a model id; use provider/model or group/id"
            ))
        })?;
        let provider = config.provider(provider_id).ok_or_else(|| {
            ApiError::NotFound(format!("there is no provider called {provider_id}"))
        })?;
        if !provider.enabled {
            return Err(ApiError::BadRequest(format!(
                "the provider {provider_id} is turned off"
            )));
        }
        if !provider.models.iter().any(|m| m == model) {
            return Err(ApiError::NotFound(format!(
                "the provider {provider_id} does not serve {model}"
            )));
        }
        let targets = targets_for(provider, model, requested);
        if targets.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "the provider {provider_id} has no usable key"
            )));
        }
        let reason = format!("{provider_id} serves {model} directly");
        Ok(RoutePlan {
            targets,
            group_id: None,
            members: vec![requested.to_string()],
            routing: None,
            reason,
        })
    }

    async fn plan_group(
        &self,
        config: &Config,
        group: &Group,
        session: Option<&str>,
    ) -> Result<RoutePlan, ApiError> {
        if group.models.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "the group {} has no members",
                group.id
            )));
        }

        // Expand nested groups into their members, keeping the order.
        let mut members: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for member in &group.models {
            expand_member(config, member, &mut members, &mut seen, 0);
        }
        if members.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "the group {} has no members that resolve",
                group.id
            )));
        }

        // Order the members the way the group's routing says.
        let members = match group.routing {
            Routing::Rotate => {
                let mut state = self.state.lock().await;
                let cursor = state.rotate.entry(group.id.clone()).or_insert(0);
                let n = members.len();
                let offset = (*cursor as usize) % n;
                *cursor = cursor.wrapping_add(1);
                let mut out = members.clone();
                out.rotate_left(offset);
                out
            }
            Routing::Usage | Routing::Smart => {
                let state = self.state.lock().await;
                let mut scored: Vec<(u64, usize, String)> = members
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (state.usage.get(m).copied().unwrap_or(0), i, m.clone()))
                    .collect();
                // Least used first; the listed order breaks a tie.
                scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
                scored.into_iter().map(|(_, _, m)| m).collect()
            }
            Routing::Weight => {
                let state = self.state.lock().await;
                weighted_order(
                    &members,
                    &group.weights,
                    state.usage.get("__rotate__").copied(),
                )
            }
            Routing::Order => members,
        };

        // Build the targets, member by member.
        let mut targets: Vec<Target> = Vec::new();
        for member in &members {
            if let Some((provider_id, model)) = Config::split_model(member) {
                if let Some(provider) = config.provider(provider_id) {
                    if provider.enabled {
                        targets.extend(targets_for(provider, model, member));
                    }
                }
            }
        }
        if targets.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "no member of the group {} could be reached",
                group.id
            )));
        }

        // A conversation stays with the target that answered it.
        let mut reason = format!(
            "the group {} routes by {}",
            group.id,
            group.routing.as_str()
        );
        if !matches!(group.stays, crate::config::Stays::Off) {
            if let Some(session) = session {
                if let Some(previous) = self.affinity_of(session).await {
                    if let Some(pos) = targets.iter().position(|t| t.key_id == previous) {
                        let target = targets.remove(pos);
                        targets.insert(0, target);
                        reason.push_str("; the conversation stays where it was");
                    }
                }
            }
        }

        Ok(RoutePlan {
            targets,
            group_id: Some(group.id.clone()),
            members,
            routing: Some(group.routing),
            reason,
        })
    }
}

/// Resolve one member into the models it stands for.
fn expand_member(
    config: &Config,
    member: &str,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
    depth: usize,
) {
    if depth > 4 || !seen.insert(member.to_string()) {
        return;
    }
    if let Some(group_id) = member.strip_prefix("group/") {
        if let Some(group) = config.group(group_id) {
            for inner in &group.models {
                expand_member(config, inner, out, seen, depth + 1);
            }
        }
        return;
    }
    out.push(member.to_string());
}

/// Every key of a provider that could serve a model.
fn targets_for(provider: &Provider, model: &str, asked: &str) -> Vec<Target> {
    provider
        .enabled_keys()
        .map(|k| Target {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            key_id: k.id.clone(),
            key_label: k.label.clone(),
            secret: k.key.clone(),
            model: model.to_string(),
            protocol: provider.protocol,
            asked: asked.to_string(),
        })
        .collect()
}

/// A weighted interleave: `a=3` gives a three requests for every one b takes.
pub fn weighted_order(
    members: &[String],
    weights: &BTreeMap<String, u32>,
    _cursor: Option<u64>,
) -> Vec<String> {
    if weights.is_empty() {
        return members.to_vec();
    }
    let mut pools: Vec<(String, u32)> = Vec::new();
    for m in members {
        let w = weights.get(m).copied().unwrap_or(1).clamp(1, 1000);
        pools.push((m.clone(), w));
    }
    let mut out = Vec::new();
    let mut remaining: Vec<(String, u32)> = pools;
    // The smoothest interleave: always take from the pool with the most
    // remaining weight relative to the others.
    while remaining.iter().any(|(_, w)| *w > 0) {
        let total: u32 = remaining.iter().map(|(_, w)| *w).sum();
        let mut best = 0usize;
        let mut best_score = f64::MIN;
        for (i, (_, w)) in remaining.iter().enumerate() {
            if *w == 0 {
                continue;
            }
            let score = (*w as f64) / (total as f64);
            if score > best_score {
                best_score = score;
                best = i;
            }
        }
        out.push(remaining[best].0.clone());
        remaining[best].1 -= 1;
        let total: u32 = remaining.iter().map(|(_, w)| *w).sum();
        if total == 0 {
            break;
        }
    }
    out
}
