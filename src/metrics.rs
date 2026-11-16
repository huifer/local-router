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

/// Push the number of providers whose usage windows are closed right now.
pub fn set_schedules_closed(n: u64) {
    gauge!("local_router_schedules_closed").set(n as f64);
}

/// Push the gateway's own process readings, as the sampler measured them.
///
/// CPU is percent of one core (`top` semantics), so a busy gateway reading
/// `200` is using two cores. `local_router_process_resident_bytes` is the
/// RSS, not a virtual size: it is the number that grows when something
/// leaks.
pub fn set_process(cpu_pct: f64, rss_bytes: u64, threads: u32) {
    gauge!("local_router_process_cpu_percent").set(cpu_pct);
    gauge!("local_router_process_resident_bytes").set(rss_bytes as f64);
    gauge!("local_router_process_threads").set(threads as f64);
}

/// Push the six-sample (one minute) request rate the sampler measured.
pub fn set_requests_per_minute(n: f64) {
    gauge!("local_router_requests_per_minute").set(n);
}

/// Count one target skipped because its usage window is closed. A local
/// policy, not a vendor 429: no cooldown follows it.
pub fn record_schedule_skipped(provider: &str) {
    counter!("local_router_schedule_skipped_total", "provider" => provider.to_string())
        .increment(1);
}

/// Count one request refused by a local rate limit.
pub fn record_rate_limited(scope: &str) {
    counter!("local_router_rate_limited_total", "scope" => scope.to_string()).increment(1);
}

/// Record one request's wall-clock duration, in seconds.
pub fn record_latency(api: &str, seconds: f64) {
    histogram!("local_router_request_duration_seconds", "api" => api.to_string()).record(seconds);
}

/// Record time to first token for one streaming reply.
///
/// `provider` is an open label (see the cardinality policy above): the whole
/// point of this histogram is comparing how fast each vendor starts talking.
/// It only receives samples from replies that actually streamed, so the
/// per-provider quantiles are per-provider *streaming* latencies.
pub fn record_ttft(api: &str, provider: &str, seconds: f64) {
    histogram!(
        "local_router_ttft_seconds",
        "api" => api.to_string(),
        "provider" => provider.to_string(),
    )
    .record(seconds);
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
    if rec.cache_write_tokens > 0 {
        counter!(
            "local_router_tokens_total",
            "kind" => "cache_write".to_string(),
            "api" => api.clone(),
            "provider" => provider.clone(),
        )
        .increment(rec.cache_write_tokens);
    }
    if let Some(cost) = rec.cost_usd.filter(|c| c.is_finite() && *c > 0.0) {
        // Counters are integers: micro-dollars keep fractions without a
        // float-typed counter. Divide by 1e6 in Grafana.
        counter!(
            "local_router_cost_micro_usd_total",
            "api" => api.clone(),
            "provider" => provider.clone(),
        )
        .increment((cost * 1_000_000.0).round() as u64);
    }
    if let Some(ttft) = rec.ttft_ms {
        record_ttft(&api, &provider, ttft as f64 / 1000.0);
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
            .find(|l| {
                l.starts_with("local_router_requests_total") && l.contains("provider=\"openai\"")
            })
            .expect("request counter line");
        let value: u64 = line.rsplit(' ').next().unwrap().parse().unwrap();
        assert!(value >= 2, "expected at least 2 attempts, got {value}");
    }

    #[tokio::test]
    async fn a_priced_request_reaches_the_micro_usd_cost_counter() {
        install();
        let mut rec = UsageRecord::new("glm-4.7", "openai_chat");
        rec.provider = "zhipu".to_string();
        rec.status = 200;
        rec.cost_usd = Some(0.25);
        record_request(&rec);

        let out = render();
        let line = out
            .lines()
            .find(|l| l.starts_with("local_router_cost_micro_usd_total{"))
            .expect("cost counter vanished from /metrics");
        let value: u64 = line.rsplit(' ').next().unwrap().parse().unwrap();
        assert_eq!(value, 250_000);
    }

    #[tokio::test]
    async fn a_streaming_reply_reaches_the_ttft_histogram_per_provider() {
        install();
        // A streaming reply carries a measured first token; it must show up
        // in `/metrics` labeled by provider, not just by api.
        let mut rec = UsageRecord::new("glm-4.7", "openai_chat");
        rec.provider = "zhipu".to_string();
        rec.status = 200;
        rec.ttft_ms = Some(56);
        record_request(&rec);

        let out = render();
        assert!(
            out.contains("local_router_ttft_seconds"),
            "streaming reply did not reach the TTFT histogram:\n{out}"
        );
        assert!(
            out.contains("provider=\"zhipu\""),
            "TTFT histogram lost the provider label:\n{out}"
        );
    }
}
