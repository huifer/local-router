//! Prometheus metrics for the local gateway.
//!
//! Telemetry *data* already exists (`usage.db` + `trace_spans`); this module
//! adds the **real-time** export endpoint. Long-term accounting stays in
//! `usage.db` — the counters here are live/perishable and must never become a
//! second source of truth. See `docs/ROADMAP-observability-control.md`.
//!
//! Cardinality policy (deliberately small):
//!   - open labels: `api`, `provider`, `status_class`
//!   - `caller_key` appears only as `has_key = "true" | "false"`
//!   - `model` is NEVER a label (it has too many values)
//!   - token counters share `api` + `provider` plus a `kind` label
//!
//! The global recorder is installed once (see `install`); when it is not
//! installed (unit tests, headless helpers) the `metrics` macros safely
//! no-op, so metric calls cost nothing and never panic.

use std::sync::OnceLock;

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusHandle;

use crate::gateway::usage::UsageRecord;

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global Prometheus recorder. Safe to call more than once; only
/// the first call takes effect. Call it once at process start, before
/// serving traffic.
pub fn install() {
    if HANDLE.get().is_some() {
        return;
    }
    match metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder() {
        Ok(handle) => {
            let _ = HANDLE.set(handle);
        }
        Err(err) => {
            tracing::warn!("could not install the Prometheus metrics recorder: {err}");
        }
    }
}

/// Render every registered metric in Prometheus text exposition format.
pub fn render() -> String {
    match HANDLE.get() {
        Some(handle) => handle.render(),
        None => String::new(),
    }
}

/// Push the current in-flight request count into a gauge.
pub fn set_in_flight(n: u64) {
    gauge!("local_router_in_flight").set(n as f64);
}

/// Push the number of providers/accounts currently in 429 cooldown.
pub fn set_cooldowns(n: u64) {
    gauge!("local_router_cooldowns_active").set(n as f64);
}

/// Push the number of scopes that carry a local rate limit.
pub fn set_rate_limits(n: u64) {
    gauge!("local_router_rate_limits_active").set(n as f64);
}

/// Count one request refused by a local rate limit.
pub fn record_rate_limited(scope: &str) {
    counter!("local_router_rate_limited_total", "scope" => scope.to_string()).increment(1);
}

/// Record one request's wall-clock duration, in seconds.
pub fn record_latency(api: &str, seconds: f64) {
    histogram!("local_router_request_duration_seconds", "api" => api.to_string()).record(seconds);
}

/// Record one persisted usage record as a request counter (and its tokens).
///
/// One logical request may produce several records (one per upstream attempt
/// in a failover chain), so this counts *attempts* — which is the more useful
/// signal for spotting 429 storms and retries.
pub fn record_request(rec: &UsageRecord) {
    let api = rec.protocol.clone();
    let provider = if rec.provider.is_empty() {
        "none".to_string()
    } else {
        rec.provider.clone()
    };
    let status_class = status_class(rec.status);
    let has_key = if rec.caller_key_id.is_some() {
        "true"
    } else {
        "false"
    };

    counter!(
        "local_router_requests_total",
        "api" => api.clone(),
        "provider" => provider.clone(),
        "status_class" => status_class.to_string(),
        "has_key" => has_key.to_string(),
    )
    .increment(1);

    if rec.input_tokens > 0 {
        counter!(
            "local_router_tokens_total",
            "kind" => "input".to_string(),
            "api" => api.clone(),
            "provider" => provider.clone(),
        )
        .increment(rec.input_tokens);
    }
    if rec.output_tokens > 0 {
        counter!(
            "local_router_tokens_total",
            "kind" => "output".to_string(),
            "api" => api.clone(),
            "provider" => provider.clone(),
        )
        .increment(rec.output_tokens);
    }
    if rec.cache_read_tokens > 0 {
        counter!(
            "local_router_tokens_total",
            "kind" => "cache_read".to_string(),
            "api" => api.clone(),
            "provider" => provider.clone(),
        )
        .increment(rec.cache_read_tokens);
    }
}

/// Bucket an HTTP status into a coarse class for the label set.
fn status_class(status: u16) -> &'static str {
    match status {
        0 => "none",
        200..=299 => "2xx",
        429 => "429",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_and_token_counters_render_as_prometheus() {
        install();
        let mut rec = UsageRecord::new("gpt-4o", "openai_chat");
        rec.provider = "openai".to_string();
        rec.caller_key_id = Some("ck".to_string());
        rec.status = 200;
        rec.input_tokens = 10;
        rec.output_tokens = 20;
        record_request(&rec);
        record_request(&rec);

        let out = render();
        assert!(
            out.contains("local_router_requests_total"),
            "missing request counter:\n{out}"
        );
        assert!(
            out.contains("local_router_tokens_total"),
            "missing token counter:\n{out}"
        );
        let line = out
            .lines()
            .find(|l| l.starts_with("local_router_requests_total"))
            .expect("request counter line");
        let value: u64 = line.rsplit(' ').next().unwrap().parse().unwrap();
        assert!(value >= 2, "expected at least 2 attempts, got {value}");
    }
}
