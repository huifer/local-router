//! Trace: the thread that ties a session to the vendor calls it made.
//!
//! A gateway is the one place that sees both ends of a conversation: the
//! agent's request (which carries a session id) and the vendor's answer.
//! What it used to throw away was the link between the two — which vendor
//! call belongs to which step of which session. Without that link a bill
//! cannot be pushed back onto a task, a wrong answer cannot be traced to
//! the call that produced it, and a vendor's own log cannot be read back
//! into ours.
//!
//! This module is that link, in the shape of a span tree:
//!
//! ```text
//! trace  trc_…            one task (the caller's own id, or the session)
//!  └─ session  ses_…      the agent's session
//!     └─ request  req_…   one request the agent made
//!         └─ upstream up_…  one attempt at one vendor
//! ```
//!
//! The identifiers are written into the request that goes upstream, so a
//! vendor — or a relay in front of it — can hand the id back, and the call
//! can be found again from the vendor's side.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The header that carries the trace id.
pub const TRACE_HEADER: &str = "x-trace-id";
/// The header that carries the request's own span id.
pub const SPAN_HEADER: &str = "x-span-id";
/// The header that carries the caller's session id.
pub const SESSION_HEADER: &str = "x-session-id";
/// The header that carries the caller's own id for this request.
pub const REQUEST_HEADER: &str = "x-request-id";
/// The header an orchestrator uses to name the sub-task a request is.
pub const TASK_HEADER: &str = "x-task-id";

/// The longest an id may be when it comes from a caller.
const MAX_ID_LEN: usize = 128;

/// What a span is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// The session: the top of the tree.
    Session,
    /// One request the agent made.
    Request,
    /// One attempt at one upstream vendor.
    Upstream,
}

impl SpanKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SpanKind::Session => "session",
            SpanKind::Request => "request",
            SpanKind::Upstream => "upstream",
        }
    }
}

/// One node of the trace tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    /// The trace this span belongs to.
    pub trace_id: String,
    /// This span's own id.
    pub span_id: String,
    /// The span this one hangs under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// What this span is.
    pub kind: SpanKind,
    /// A short label: the model asked for, the provider tried.
    #[serde(default)]
    pub name: String,
    /// The caller's session, when it sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The caller's own name for this step, when it sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The agent that made the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The project the request ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The local key that authorized the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_key_id: Option<String>,
    /// The model the caller asked for.
    #[serde(default)]
    pub model: String,
    /// The provider that answered (or refused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The provider key that answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_key_id: Option<String>,
    /// The protocol the vendor spoke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Which attempt this was: 1 is the first try.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// How many requests the session made (session spans only).
    #[serde(default)]
    pub requests: u64,
    /// The HTTP status the vendor answered with.
    #[serde(default)]
    pub status: u16,
    /// Whether the call succeeded.
    #[serde(default = "yes")]
    pub ok: bool,
    /// Whether the attempt was never made: the provider was skipped
    /// (cooling down, out of quota). The request carried on without it.
    #[serde(default)]
    pub skipped: bool,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How long it took, in milliseconds.
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// What the call cost in US dollars, when the model's price is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// What it cost in the vendor's own credits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<f64>,
    /// When the span started, RFC 3339.
    pub started_at: String,
    /// When it ended, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// The vendor's own id for the call, when it reported one. This is the
    /// id that shows up in the vendor's log, and the way back from there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_request_id: Option<String>,
    /// How many messages the request carried.
    #[serde(default)]
    pub messages: u32,
    /// The opening line of the first user message, for a human to recognise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

fn yes() -> bool {
    true
}

impl Span {
    /// A new span of some kind, with the clock already running.
    pub fn start(kind: SpanKind, trace_id: &str, span_id: &str) -> Span {
        Span {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: None,
            kind,
            name: String::new(),
            session_id: None,
            task_id: None,
            agent: None,
            project: None,
            caller_key_id: None,
            model: String::new(),
            provider: None,
            provider_key_id: None,
            protocol: None,
            attempt: None,
            requests: 0,
            status: 0,
            ok: true,
            skipped: false,
            error: None,
            latency_ms: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_usd: None,
            credits: None,
            started_at: now(),
            ended_at: None,
            upstream_request_id: None,
            messages: 0,
            preview: None,
        }
    }

    /// Close the span: the clock stops, the answer is known.
    pub fn finish(&mut self, latency_ms: u64, ok: bool, status: u16, error: Option<String>) {
        self.latency_ms = latency_ms;
        self.ok = ok;
        self.status = status;
        self.error = error;
        self.ended_at = Some(now());
    }
}

/// The identity a request carries through the gateway.
#[derive(Debug, Clone)]
pub struct TraceContext {
    /// The trace this request belongs to.
    pub trace_id: String,
    /// The request's own span id.
    pub span_id: String,
    /// The session span this request hangs under, when there is a session.
    pub session_span_id: Option<String>,
    /// The caller's session id.
    pub session_id: Option<String>,
    /// The caller's own name for this step.
    pub task_id: Option<String>,
    /// The caller's own id for this request.
    pub request_id: Option<String>,
    /// Whether the trace id came from the caller.
    pub from_caller: bool,
}

impl TraceContext {
    /// Work out who this request belongs to.
    ///
    /// The caller's own trace id wins, so an orchestrator that already keeps
    /// a trace can hand it down and the gateway will join its tree. Without
    /// one, the session is the trace: every request of a session belongs to
    /// the same task, which is exactly what the session *is*. A request with
    /// neither gets a trace of its own.
    pub fn resolve(body: &Value, headers: &HeaderMap, session: Option<&str>) -> TraceContext {
        let session_id = session
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let asked = header(headers, TRACE_HEADER)
            .or_else(|| body_field(body, "trace_id"))
            .and_then(|v| sanitize(&v));
        let from_caller = asked.is_some();
        let trace_id = match asked {
            Some(id) => id,
            None => match &session_id {
                Some(session) => format!("trc_{}", digest(session)),
                None => format!("trc_{}", random_hex()),
            },
        };

        let task_id = header(headers, TASK_HEADER)
            .or_else(|| body_field(body, "task_id"))
            .and_then(|v| sanitize(&v));
        let request_id = header(headers, REQUEST_HEADER)
            .or_else(|| body_field(body, "request_id"))
            .and_then(|v| sanitize(&v));

        let session_span_id = session_id
            .as_ref()
            .map(|session| format!("ses_{}", digest(&format!("{session}\u{1}{trace_id}"))));

        TraceContext {
            trace_id,
            span_id: format!("req_{}", random_hex()),
            session_span_id,
            session_id,
            task_id,
            request_id,
            from_caller,
        }
    }

    /// The parent of this request's span: the session, when there is one.
    pub fn parent_span_id(&self) -> Option<String> {
        self.session_span_id.clone()
    }
}

/// A span and the spans under it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceNode {
    pub span: Span,
    pub children: Vec<TraceNode>,
}

/// One trace, as the list view shows it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraceSummary {
    pub trace_id: String,
    /// The sessions that took part.
    pub sessions: Vec<String>,
    /// The agents that took part.
    pub agents: Vec<String>,
    /// The projects the work happened in.
    pub projects: Vec<String>,
    /// The models the callers asked for.
    pub models: Vec<String>,
    /// The vendors that answered.
    pub providers: Vec<String>,
    /// How many requests the trace holds.
    pub requests: u64,
    /// How many upstream calls it took.
    pub upstream_calls: u64,
    /// How many of them failed.
    pub failed: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub credits: f64,
    /// The time spent in vendor calls.
    pub latency_ms: u64,
    /// The wall-clock length of the trace.
    pub duration_ms: u64,
    pub started_at: String,
    pub ended_at: String,
    pub ok: bool,
}

/// Summarise every trace in a set of spans, newest first.
pub fn summarize(spans: &[Span]) -> Vec<TraceSummary> {
    let mut traces: BTreeMap<String, Vec<&Span>> = BTreeMap::new();
    for span in spans {
        traces.entry(span.trace_id.clone()).or_default().push(span);
    }
    let mut out: Vec<TraceSummary> = traces
        .into_iter()
        .map(|(trace_id, spans)| summary_of(&trace_id, &spans))
        .collect();
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    out
}

fn summary_of(trace_id: &str, spans: &[&Span]) -> TraceSummary {
    let mut s = TraceSummary {
        trace_id: trace_id.to_string(),
        ok: true,
        ..Default::default()
    };
    let mut sessions = BTreeSet::new();
    let mut agents = BTreeSet::new();
    let mut projects = BTreeSet::new();
    let mut models = BTreeSet::new();
    let mut providers = BTreeSet::new();
    let mut first: Option<String> = None;
    let mut last: Option<String> = None;

    for span in spans {
        if let Some(v) = &span.session_id {
            sessions.insert(v.clone());
        }
        if let Some(v) = &span.agent {
            agents.insert(v.clone());
        }
        if let Some(v) = &span.project {
            projects.insert(v.clone());
        }
        if !span.model.is_empty() {
            models.insert(span.model.clone());
        }
        if let Some(v) = &span.provider {
            providers.insert(v.clone());
        }
        if span.kind == SpanKind::Request {
            s.requests += 1;
        }
        if span.kind == SpanKind::Upstream {
            s.upstream_calls += 1;
            s.input_tokens += span.input_tokens;
            s.output_tokens += span.output_tokens;
            s.cache_read_tokens += span.cache_read_tokens;
            s.latency_ms += span.latency_ms;
            if let Some(cost) = span.cost_usd {
                s.cost_usd += cost;
            }
            if let Some(credits) = span.credits {
                s.credits += credits;
            }
        }
        if !span.ok && !span.skipped {
            s.failed += 1;
            s.ok = false;
        }
        first = Some(match first {
            Some(current) if current <= span.started_at => current,
            _ => span.started_at.clone(),
        });
        let ended = span
            .ended_at
            .clone()
            .unwrap_or_else(|| span.started_at.clone());
        last = Some(match last {
            Some(current) if current >= ended => current,
            _ => ended,
        });
    }

    s.sessions = sessions.into_iter().collect();
    s.agents = agents.into_iter().collect();
    s.projects = projects.into_iter().collect();
    s.models = models.into_iter().collect();
    s.providers = providers.into_iter().collect();
    s.started_at = first.unwrap_or_default();
    s.ended_at = last.unwrap_or_default();
    s.duration_ms = duration_between(&s.started_at, &s.ended_at);
    s
}

/// Build the tree of a trace: the roots first, children under them.
pub fn tree(spans: &[Span]) -> Vec<TraceNode> {
    let mut by_id: BTreeMap<&str, &Span> = BTreeMap::new();
    for span in spans {
        by_id.insert(span.span_id.as_str(), span);
    }
    // Children first, so a node can be moved into its parent.
    let mut children: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
    let mut roots: Vec<&Span> = Vec::new();
    for span in spans {
        match span
            .parent_span_id
            .as_deref()
            .filter(|parent| by_id.contains_key(parent))
        {
            Some(parent) => children.entry(parent).or_default().push(span),
            None => roots.push(span),
        }
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    }
    roots.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    roots
        .into_iter()
        .map(|span| subtree(span, &children))
        .collect()
}

/// The spans of one trace, oldest first.
pub fn trace_spans(spans: &[Span], trace_id: &str) -> Vec<Span> {
    let mut out: Vec<Span> = spans
        .iter()
        .filter(|s| s.trace_id == trace_id)
        .cloned()
        .collect();
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    out
}

/// The spans of one session, oldest first.
pub fn session_spans(spans: &[Span], session_id: &str) -> Vec<Span> {
    let mut out: Vec<Span> = spans
        .iter()
        .filter(|s| s.session_id.as_deref() == Some(session_id))
        .cloned()
        .collect();
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    out
}

/// One session, as the session list shows it.
///
/// A session is the whole story: every request the agent made under one id,
/// and every vendor call those took — no matter which trace id the caller
/// gave each of them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    /// The conversation's title, filled in by the caller when it is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The trace ids the session's requests were filed under.
    pub traces: Vec<String>,
    pub agents: Vec<String>,
    pub projects: Vec<String>,
    pub models: Vec<String>,
    pub providers: Vec<String>,
    pub requests: u64,
    pub upstream_calls: u64,
    pub failed: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub credits: f64,
    /// The time spent in vendor calls.
    pub latency_ms: u64,
    /// The wall-clock length, from the first request to the last answer.
    pub duration_ms: u64,
    pub started_at: String,
    pub ended_at: String,
    pub ok: bool,
}

/// Summarise every session in a set of spans, newest first.
pub fn summarize_sessions(spans: &[Span]) -> Vec<SessionSummary> {
    let mut sessions: BTreeMap<String, Vec<&Span>> = BTreeMap::new();
    for span in spans {
        if let Some(session) = &span.session_id {
            if !session.is_empty() {
                sessions.entry(session.clone()).or_default().push(span);
            }
        }
    }
    let mut out: Vec<SessionSummary> = sessions
        .into_iter()
        .map(|(session_id, spans)| session_summary_of(&session_id, &spans))
        .collect();
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    out
}

fn session_summary_of(session_id: &str, spans: &[&Span]) -> SessionSummary {
    let mut s = SessionSummary {
        session_id: session_id.to_string(),
        ok: true,
        ..Default::default()
    };
    let mut traces = BTreeSet::new();
    let mut agents = BTreeSet::new();
    let mut projects = BTreeSet::new();
    let mut models = BTreeSet::new();
    let mut providers = BTreeSet::new();
    let mut first: Option<String> = None;
    let mut last: Option<String> = None;

    for span in spans {
        traces.insert(span.trace_id.clone());
        if let Some(v) = &span.agent {
            agents.insert(v.clone());
        }
        if let Some(v) = &span.project {
            projects.insert(v.clone());
        }
        if !span.model.is_empty() {
            models.insert(span.model.clone());
        }
        if let Some(v) = &span.provider {
            providers.insert(v.clone());
        }
        if span.kind == SpanKind::Request {
            s.requests += 1;
        }
        if span.kind == SpanKind::Upstream {
            s.upstream_calls += 1;
            s.input_tokens += span.input_tokens;
            s.output_tokens += span.output_tokens;
            s.cache_read_tokens += span.cache_read_tokens;
            s.latency_ms += span.latency_ms;
            if let Some(cost) = span.cost_usd {
                s.cost_usd += cost;
            }
            if let Some(credits) = span.credits {
                s.credits += credits;
            }
        }
        if !span.ok && !span.skipped {
            s.failed += 1;
            s.ok = false;
        }
        first = Some(match first {
            Some(current) if current <= span.started_at => current,
            _ => span.started_at.clone(),
        });
        let ended = span
            .ended_at
            .clone()
            .unwrap_or_else(|| span.started_at.clone());
        last = Some(match last {
            Some(current) if current >= ended => current,
            _ => ended,
        });
    }

    s.traces = traces.into_iter().collect();
    s.agents = agents.into_iter().collect();
    s.projects = projects.into_iter().collect();
    s.models = models.into_iter().collect();
    s.providers = providers.into_iter().collect();
    s.started_at = first.unwrap_or_default();
    s.ended_at = last.unwrap_or_default();
    s.duration_ms = duration_between(&s.started_at, &s.ended_at);
    s
}

/// The whole chain of one session: every request it made and every vendor
/// call those took, gathered from every trace id they were filed under.
///
/// The root is the session itself; the requests hang under it in the order
/// they were made, each with the vendor calls it produced.
pub fn session_view(spans: &[Span], session_id: &str) -> Option<TraceNode> {
    let mine = session_spans(spans, session_id);
    if mine.is_empty() {
        return None;
    }
    // The children of every span, so a subtree can be walked from its root.
    let mut by_parent: BTreeMap<&str, Vec<&Span>> = BTreeMap::new();
    for span in &mine {
        if let Some(parent) = &span.parent_span_id {
            by_parent.entry(parent.as_str()).or_default().push(span);
        }
    }
    for list in by_parent.values_mut() {
        list.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    }

    let requests: Vec<&Span> = mine
        .iter()
        .filter(|s| s.kind == SpanKind::Request)
        .collect();
    // The session node is built here, not read from the log: a session whose
    // requests carry several trace ids has several session spans, and the
    // view must still be one tree.
    let first = mine.first().cloned()?;
    let mut root = Span::start(
        SpanKind::Session,
        &first.trace_id,
        &format!("ses_{}", digest(session_id)),
    );
    root.name = session_id.to_string();
    root.session_id = Some(session_id.to_string());
    root.agent = first.agent.clone();
    root.project = first.project.clone();
    root.started_at = first.started_at.clone();
    root.ended_at = mine
        .last()
        .and_then(|s| s.ended_at.clone())
        .or_else(|| mine.last().map(|s| s.started_at.clone()));
    root.requests = requests.len() as u64;
    // The vendor calls answer for the tokens and the money.
    for span in &mine {
        if span.kind == SpanKind::Upstream {
            root.latency_ms += span.latency_ms;
            root.input_tokens += span.input_tokens;
            root.output_tokens += span.output_tokens;
            if let Some(cost) = span.cost_usd {
                root.cost_usd = Some(root.cost_usd.unwrap_or(0.0) + cost);
            }
            if let Some(credits) = span.credits {
                root.credits = Some(root.credits.unwrap_or(0.0) + credits);
            }
        }
        if !span.ok && !span.skipped {
            root.ok = false;
        }
    }

    Some(TraceNode {
        span: root,
        children: requests
            .iter()
            .map(|request| subtree(request, &by_parent))
            .collect(),
    })
}

/// A span and everything under it.
fn subtree(span: &Span, by_parent: &BTreeMap<&str, Vec<&Span>>) -> TraceNode {
    TraceNode {
        span: span.clone(),
        children: by_parent
            .get(span.span_id.as_str())
            .map(|list| list.iter().map(|child| subtree(child, by_parent)).collect())
            .unwrap_or_default(),
    }
}

/// The spans that carry a vendor's own request id — the way back from a
/// vendor's log to the session that caused the call.
pub fn spans_by_upstream_id(spans: &[Span], upstream_request_id: &str) -> Vec<Span> {
    spans
        .iter()
        .filter(|s| {
            s.upstream_request_id
                .as_deref()
                .map(|id| id == upstream_request_id)
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Every span, as a csv an auditor can open.
pub fn to_csv(spans: &[Span]) -> String {
    let mut out = String::new();
    out.push_str("trace_id,span_id,parent_span_id,kind,name,started_at,ended_at,latency_ms,session_id,task_id,agent,project,model,provider,provider_key_id,protocol,attempt,status,ok,skipped,error,input_tokens,output_tokens,cache_read_tokens,cost_usd,credits,upstream_request_id\n");
    for span in spans {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            csv_field(&span.trace_id),
            csv_field(&span.span_id),
            csv_field(span.parent_span_id.as_deref().unwrap_or("")),
            span.kind.as_str(),
            csv_field(&span.name),
            csv_field(&span.started_at),
            csv_field(span.ended_at.as_deref().unwrap_or("")),
            span.latency_ms,
            csv_field(span.session_id.as_deref().unwrap_or("")),
            csv_field(span.task_id.as_deref().unwrap_or("")),
            csv_field(span.agent.as_deref().unwrap_or("")),
            csv_field(span.project.as_deref().unwrap_or("")),
            csv_field(&span.model),
            csv_field(span.provider.as_deref().unwrap_or("")),
            csv_field(span.provider_key_id.as_deref().unwrap_or("")),
            csv_field(span.protocol.as_deref().unwrap_or("")),
            span.attempt.map(|a| a.to_string()).unwrap_or_default(),
            span.status,
            span.ok,
            span.skipped,
            csv_field(span.error.as_deref().unwrap_or("")),
            span.input_tokens,
            span.output_tokens,
            span.cache_read_tokens,
            span.cost_usd.map(|c| format!("{c:.6}")).unwrap_or_default(),
            span.credits.map(|c| format!("{c:.4}")).unwrap_or_default(),
            csv_field(span.upstream_request_id.as_deref().unwrap_or("")),
        ));
    }
    out
}

/// Quote a csv field when it needs it.
fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

// -------------------------------------------------------------- the log --

/// How large the trace log may grow before it is rolled over.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// The limit the log is rolled over at. The environment may lower it, which
/// is what the tests do.
fn max_log_bytes() -> u64 {
    std::env::var("LOCAL_ROUTER_TRACE_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(MAX_LOG_BYTES)
}

/// Append one span to the log.
pub fn record(path: &Path, span: &Span) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Keep the log from growing forever: the old file becomes one
    // generation of history, and the new one starts fresh.
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > max_log_bytes() {
            let old = path.with_extension("jsonl.1");
            let _ = std::fs::rename(path, old);
        }
    }
    let Ok(line) = serde_json::to_string(span) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Read every span back, oldest first.
pub fn read_all(path: &Path) -> Vec<Span> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Span>(l).ok())
        .collect()
}

// ------------------------------------------------------------- helpers --

/// The current time, RFC 3339.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// A fresh random hex id.
pub fn random_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// A short, stable digest of a string — the same input always gives the
/// same id, so a session keeps its trace across restarts.
pub fn digest(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// Keep only the characters an id may carry, and cap its length.
pub fn sanitize(value: &str) -> Option<String> {
    let cleaned: String = value
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        .take(MAX_ID_LEN)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// A header's value, trimmed.
fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// A body field, in `metadata` or at the top level.
fn body_field(body: &Value, name: &str) -> Option<String> {
    body.pointer(&format!("/metadata/{name}"))
        .or_else(|| body.get(name))
        .and_then(Value::as_str)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The wall-clock length between two RFC 3339 stamps.
fn duration_between(start: &str, end: &str) -> u64 {
    let parse = |value: &str| chrono::DateTime::parse_from_rfc3339(value).ok();
    match (parse(start), parse(end)) {
        (Some(a), Some(b)) => (b - a).num_milliseconds().max(0) as u64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn body(value: Value) -> Value {
        value
    }

    #[test]
    fn a_callers_trace_id_is_kept_and_a_session_becomes_one_otherwise() {
        let given = TraceContext::resolve(
            &body(serde_json::json!({})),
            &headers(&[("x-trace-id", "task-42")]),
            Some("sess-1"),
        );
        assert_eq!(given.trace_id, "task-42");
        assert!(given.from_caller);
        assert_eq!(given.session_id.as_deref(), Some("sess-1"));
        assert!(given.session_span_id.is_some());

        // Without one, the session is the trace — and it stays the same
        // for every request of that session.
        let first =
            TraceContext::resolve(&body(serde_json::json!({})), &headers(&[]), Some("sess-1"));
        let second =
            TraceContext::resolve(&body(serde_json::json!({})), &headers(&[]), Some("sess-1"));
        assert_eq!(first.trace_id, second.trace_id);
        assert!(first.trace_id.starts_with("trc_"));
        assert!(!first.from_caller);
        // Each request is its own span.
        assert_ne!(first.span_id, second.span_id);
        // And the request hangs under the session.
        assert_eq!(first.parent_span_id(), first.session_span_id);
    }

    #[test]
    fn a_request_without_a_session_gets_a_trace_of_its_own() {
        let ctx = TraceContext::resolve(&body(serde_json::json!({})), &headers(&[]), None);
        assert!(ctx.trace_id.starts_with("trc_"));
        assert!(ctx.session_span_id.is_none());
        assert_eq!(ctx.parent_span_id(), None);
    }

    #[test]
    fn the_body_can_carry_the_trace_and_task_ids_too() {
        let ctx = TraceContext::resolve(
            &body(serde_json::json!({
                "metadata": {"trace_id": "from-body", "task_id": "sub-agent-2"}
            })),
            &headers(&[]),
            Some("sess-1"),
        );
        assert_eq!(ctx.trace_id, "from-body");
        assert_eq!(ctx.task_id.as_deref(), Some("sub-agent-2"));
    }

    #[test]
    fn a_caller_cannot_smuggle_a_messy_id_into_the_log() {
        let messy = "bad id, with; breaks & a very long tail that goes on and on";
        let ctx = TraceContext::resolve(
            &body(serde_json::json!({})),
            &headers(&[("x-trace-id", messy)]),
            None,
        );
        assert!(!ctx.trace_id.contains(' '));
        assert!(!ctx.trace_id.contains(','));
        assert!(!ctx.trace_id.contains(';'));
        assert!(!ctx.trace_id.contains('&'));
        assert!(ctx.trace_id.len() <= MAX_ID_LEN);
    }

    #[test]
    fn a_span_starts_open_and_closes_with_the_answer() {
        let mut span = Span::start(SpanKind::Upstream, "trc_1", "up_1");
        assert!(span.ok);
        assert!(span.ended_at.is_none());
        span.finish(42, false, 429, Some("slow down".into()));
        assert!(!span.ok);
        assert_eq!(span.status, 429);
        assert_eq!(span.latency_ms, 42);
        assert_eq!(span.error.as_deref(), Some("slow down"));
        assert!(span.ended_at.is_some());
    }

    #[test]
    fn a_tree_puts_the_session_over_its_requests_and_the_vendor_calls_under() {
        let mut session = Span::start(SpanKind::Session, "trc_1", "ses_1");
        session.session_id = Some("sess-1".into());
        session.ended_at = Some(now());
        let mut request = Span::start(SpanKind::Request, "trc_1", "req_1");
        request.parent_span_id = Some("ses_1".into());
        request.session_id = Some("sess-1".into());
        request.ended_at = Some(now());
        let mut upstream = Span::start(SpanKind::Upstream, "trc_1", "up_1");
        upstream.parent_span_id = Some("req_1".into());
        upstream.provider = Some("anthropic".into());
        upstream.input_tokens = 10;
        upstream.output_tokens = 5;
        upstream.cost_usd = Some(0.25);
        upstream.ended_at = Some(now());

        let tree = tree(&[upstream.clone(), request.clone(), session.clone()]);
        assert_eq!(tree.len(), 1);
        let root = &tree[0];
        assert_eq!(root.span.span_id, "ses_1");
        assert_eq!(root.children.len(), 1);
        let req = &root.children[0];
        assert_eq!(req.span.span_id, "req_1");
        assert_eq!(req.children.len(), 1);
        assert_eq!(req.children[0].span.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn a_summary_counts_the_requests_the_calls_and_the_money() {
        let mut request = Span::start(SpanKind::Request, "trc_1", "req_1");
        request.session_id = Some("sess-1".into());
        request.agent = Some("claude-code".into());
        request.model = "group/main".into();
        request.ended_at = Some(now());
        let mut ok = Span::start(SpanKind::Upstream, "trc_1", "up_1");
        ok.parent_span_id = Some("req_1".into());
        ok.provider = Some("openai".into());
        ok.input_tokens = 100;
        ok.output_tokens = 20;
        ok.cost_usd = Some(0.5);
        ok.ended_at = Some(now());
        let mut bad = Span::start(SpanKind::Upstream, "trc_1", "up_2");
        bad.parent_span_id = Some("req_1".into());
        bad.provider = Some("anthropic".into());
        bad.ok = false;
        bad.error = Some("429".into());
        bad.ended_at = Some(now());

        let summaries = summarize(&[request, ok, bad]);
        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.requests, 1);
        assert_eq!(s.upstream_calls, 2);
        assert_eq!(s.failed, 1);
        assert!(!s.ok);
        assert_eq!(s.input_tokens, 100);
        assert_eq!(s.sessions, vec!["sess-1".to_string()]);
        assert_eq!(
            s.providers,
            vec!["anthropic".to_string(), "openai".to_string()]
        );
        assert!((s.cost_usd - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_skipped_attempt_is_not_a_failure() {
        let mut request = Span::start(SpanKind::Request, "trc_1", "req_1");
        request.ended_at = Some(now());
        let mut skipped = Span::start(SpanKind::Upstream, "trc_1", "up_1");
        skipped.parent_span_id = Some("req_1".into());
        skipped.provider = Some("anthropic".into());
        skipped.skipped = true;
        skipped.error = Some("the provider is cooling down after a 429".into());
        skipped.ended_at = Some(now());
        let mut served = Span::start(SpanKind::Upstream, "trc_1", "up_2");
        served.parent_span_id = Some("req_1".into());
        served.provider = Some("openai".into());
        served.ended_at = Some(now());

        let s = &summarize(&[request, skipped, served])[0];
        assert_eq!(s.upstream_calls, 2);
        // The provider that was skipped is not a failed call.
        assert_eq!(s.failed, 0);
        assert!(s.ok);
    }

    #[test]
    fn a_session_holds_every_request_even_when_the_trace_ids_differ() {
        // Two requests of one session, filed under two different trace ids
        // (the caller named its own tasks).
        let mut first = Span::start(SpanKind::Request, "task-a", "req_1");
        first.parent_span_id = Some("ses_a".into());
        first.session_id = Some("sess-1".into());
        first.model = "group/main".into();
        first.ended_at = Some(now());
        let mut call_a = Span::start(SpanKind::Upstream, "task-a", "up_1");
        call_a.parent_span_id = Some("req_1".into());
        call_a.session_id = Some("sess-1".into());
        call_a.provider = Some("openai".into());
        call_a.input_tokens = 10;
        call_a.ended_at = Some(now());

        let mut second = Span::start(SpanKind::Request, "task-b", "req_2");
        second.parent_span_id = Some("ses_b".into());
        second.session_id = Some("sess-1".into());
        second.model = "group/main".into();
        second.ended_at = Some(now());
        let mut call_b = Span::start(SpanKind::Upstream, "task-b", "up_2");
        call_b.parent_span_id = Some("req_2".into());
        call_b.session_id = Some("sess-1".into());
        call_b.provider = Some("anthropic".into());
        call_b.output_tokens = 4;
        call_b.ended_at = Some(now());

        let spans = vec![first, call_a, second, call_b];
        // The session view is one tree, whatever the trace ids were.
        let node = session_view(&spans, "sess-1").expect("the session has a chain");
        assert_eq!(node.span.kind, SpanKind::Session);
        assert_eq!(node.span.session_id.as_deref(), Some("sess-1"));
        assert_eq!(node.children.len(), 2);
        assert_eq!(node.children[0].children.len(), 1);
        assert_eq!(node.children[1].children.len(), 1);
        assert_eq!(
            node.children[0].children[0].span.provider.as_deref(),
            Some("openai")
        );
        assert_eq!(
            node.children[1].children[0].span.provider.as_deref(),
            Some("anthropic")
        );
        assert_eq!(node.span.input_tokens, 10);
        assert_eq!(node.span.output_tokens, 4);

        // And the session summary says the same thing.
        let summaries = summarize_sessions(&spans);
        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.session_id, "sess-1");
        assert_eq!(s.requests, 2);
        assert_eq!(s.upstream_calls, 2);
        assert_eq!(s.traces, vec!["task-a".to_string(), "task-b".to_string()]);
        assert_eq!(
            s.providers,
            vec!["anthropic".to_string(), "openai".to_string()]
        );
        assert_eq!(s.input_tokens, 10);
    }

    #[test]
    fn a_session_nobody_talked_to_has_no_chain() {
        let spans = vec![Span::start(SpanKind::Request, "trc_1", "req_1")];
        assert!(session_view(&spans, "sess-nope").is_none());
    }

    #[test]
    fn a_vendors_own_id_finds_the_call_back() {
        let mut span = Span::start(SpanKind::Upstream, "trc_1", "up_1");
        span.session_id = Some("sess-1".into());
        span.upstream_request_id = Some("msg_abc".into());
        let found = spans_by_upstream_id(&[span], "msg_abc");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].session_id.as_deref(), Some("sess-1"));
        assert!(spans_by_upstream_id(&[], "msg_abc").is_empty());
    }

    #[test]
    fn the_csv_keeps_a_field_with_a_comma_in_it_whole() {
        let mut span = Span::start(SpanKind::Upstream, "trc_1", "up_1");
        span.provider = Some("a,b".into());
        let csv = to_csv(&[span]);
        assert!(csv.lines().nth(1).unwrap().contains("\"a,b\""));
        assert!(csv.starts_with("trace_id,span_id"));
    }

    #[test]
    fn spans_round_trip_through_the_jsonl_log() {
        let dir = std::env::temp_dir().join(format!(
            "local-router-trace-unit-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("traces.jsonl");
        let mut span = Span::start(SpanKind::Request, "trc_1", "req_1");
        span.session_id = Some("sess-1".into());
        span.preview = Some("帮我重构网关".into());
        record(&path, &span);
        let read = read_all(&path);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].span_id, "req_1");
        assert_eq!(read[0].preview.as_deref(), Some("帮我重构网关"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
