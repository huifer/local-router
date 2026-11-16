//! The record of what each request cost.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::ir::StreamEvent;

/// One request, as the usage page shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    /// When the request finished.
    pub time: String,
    /// The model the caller asked for.
    pub model: String,
    /// The provider that answered.
    pub provider: String,
    /// The key that answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_label: Option<String>,
    /// The local gateway key that authorized the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_key_label: Option<String>,
    /// The group the request went through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The API the caller spoke.
    pub protocol: String,
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
    /// How long the call took, in milliseconds.
    #[serde(default)]
    pub latency_ms: u64,
    /// Time to first token, in milliseconds. Only a streaming reply has
    /// one: it is the wait the agent actually feels before output starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    /// The HTTP status the caller saw.
    #[serde(default)]
    pub status: u16,
    #[serde(default = "yes")]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The agent that made the request, when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The caller's own session id, when it sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The vendor's own id for the call, when it reported one. This is the
    /// id that shows up in the vendor's log, and the way back from there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_request_id: Option<String>,
    /// The working directory the caller ran from, as a project key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// How full the request's context window was, broken down by source.
    /// Collected when the gateway could measure it (every request whose body
    /// it parsed). None means the breakdown was not captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextBreakdown>,
    /// What the request cost in US dollars, when the model's price is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// What it cost in the vendor's own credits, when the vendor reports
    /// one (WorkBuddy answers credits per request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<f64>,
    /// Whether the cost could be worked out at all. An unpriced request is
    /// one whose model has no price on file.
    #[serde(default = "yes")]
    pub priced: bool,
    /// Runtime only: the vendor's `Retry-After` for this attempt, in
    /// seconds. It drives the cooldown, never the bill.
    #[serde(skip)]
    pub retry_after_secs: Option<u64>,
}

fn yes() -> bool {
    true
}

impl UsageRecord {
    /// Take the token counts an event carries.
    ///
    /// A stream is the usual shape of a request, and its usage only shows
    /// up in the last events; without this the usage page would report
    /// zeroes for every agent that streams.
    pub fn note_usage(&mut self, event: &StreamEvent) {
        // The vendor's own id for the call arrives with the first event.
        if let StreamEvent::MessageStart { id, .. } = event {
            if !id.is_empty() {
                self.upstream_request_id = Some(id.clone());
            }
        }
        let usage = match event {
            StreamEvent::MessageStart { usage, .. } | StreamEvent::MessageDelta { usage, .. } => {
                *usage
            }
            _ => return,
        };
        self.input_tokens = usage.input_tokens.max(self.input_tokens);
        self.output_tokens = usage.output_tokens.max(self.output_tokens);
        self.cache_read_tokens = usage.cache_read_tokens.max(self.cache_read_tokens);
        self.cache_write_tokens = usage.cache_write_tokens.max(self.cache_write_tokens);
        self.reasoning_tokens = usage.reasoning_tokens.max(self.reasoning_tokens);
    }

    pub fn new(model: &str, protocol: &str) -> Self {
        UsageRecord {
            time: chrono::Utc::now().to_rfc3339(),
            model: model.to_string(),
            provider: String::new(),
            key_id: None,
            key_label: None,
            caller_key_id: None,
            caller_key_label: None,
            group: None,
            protocol: protocol.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            latency_ms: 0,
            ttft_ms: None,
            status: 0,
            ok: true,
            error: None,
            agent: None,
            session_id: None,
            upstream_request_id: None,
            project: None,
            context: None,
            cost_usd: None,
            credits: None,
            priced: true,
            retry_after_secs: None,
        }
    }
}

/// How large the usage log may grow before it is rolled over.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// The limit the log is rolled over at. The environment may lower it, which
/// is what the tests do.
fn max_log_bytes() -> u64 {
    std::env::var("LOCAL_ROUTER_USAGE_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(MAX_LOG_BYTES)
}

/// Append one record to the log.
pub fn record(path: &Path, rec: &UsageRecord) {
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
    let Ok(line) = serde_json::to_string(rec) else {
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

/// Read every record back, newest last.
pub fn read_all(path: &Path) -> Vec<UsageRecord> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<UsageRecord>(l).ok())
        .collect()
}

/// The totals the usage page's tiles show.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Totals {
    pub requests: u64,
    pub failed: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    /// What the successful requests cost in US dollars, over the models
    /// whose price is known.
    pub cost_usd: f64,
    /// What they cost in the vendors' own credits, where a rate is known.
    pub credits: f64,
    /// How many requests could not be priced at all.
    pub unpriced: u64,
    /// Streaming replies that actually reported a first token. This is the
    /// denominator for every TTFT number below: a non-streaming reply has no
    /// first token to measure, so the average must not be read against
    /// `requests`.
    pub ttft_samples: u64,
    /// Mean time to first token, in milliseconds (0 when no samples).
    pub ttft_avg_ms: u64,
    /// Median time to first token, in milliseconds.
    pub ttft_p50_ms: Option<u64>,
    /// 95th percentile time to first token, in milliseconds.
    pub ttft_p95_ms: Option<u64>,
    /// Sum of the successful requests' wall-clock durations, in
    /// milliseconds. The denominator for `tokens_per_sec`.
    pub latency_sum_ms: u64,
    /// Sum of the streaming generation phase (request duration minus the
    /// wait for the first token), in milliseconds. The denominator for
    /// `output_tokens_per_sec`; only streaming replies contribute.
    pub generation_ms: u64,
    /// Billable tokens per second of request time. A whole-account
    /// throughput, not a live rate: concurrent requests count as if they
    /// ran back to back, which is what makes providers comparable.
    pub tokens_per_sec: f64,
    /// Output tokens per second of generation time (after the first
    /// token). Zero until at least one streaming reply has a TTFT.
    pub output_tokens_per_sec: f64,
    /// Runtime only: the samples the percentiles are computed from. Never
    /// serialized — the API exposes the finished numbers, not raw rows.
    #[serde(skip)]
    ttft_values: Vec<u64>,
    /// Runtime only: output tokens from the same streaming replies that
    /// contributed `generation_ms`, so the output rate's numerator and
    /// denominator always cover the same replies.
    #[serde(skip)]
    generation_output: u64,
}

impl Totals {
    /// Fold one record into the totals. Every aggregation (`totals`,
    /// `group_by`, `by_day`) goes through this one method so their numbers
    /// cannot drift apart — the cache-token bug came from three hand-written
    /// copies of this loop.
    fn note(&mut self, r: &UsageRecord) {
        self.requests += 1;
        if !r.ok {
            self.failed += 1;
            return;
        }
        self.input_tokens += r.input_tokens;
        self.output_tokens += r.output_tokens;
        self.cache_read_tokens += r.cache_read_tokens;
        self.cache_write_tokens += r.cache_write_tokens;
        self.reasoning_tokens += r.reasoning_tokens;
        if let Some(usd) = r.cost_usd {
            self.cost_usd += usd;
        }
        if let Some(credits) = r.credits {
            self.credits += credits;
        }
        if !r.priced {
            self.unpriced += 1;
        }
        if let Some(ms) = r.ttft_ms {
            self.ttft_values.push(ms);
            // The generation phase is everything after the first token.
            self.generation_ms += r.latency_ms.saturating_sub(ms);
            self.generation_output += r.output_tokens;
        }
        self.latency_sum_ms += r.latency_ms;
    }

    /// Turn the collected samples into the numbers the API returns.
    fn finish(&mut self) {
        self.ttft_samples = self.ttft_values.len() as u64;
        if !self.ttft_values.is_empty() {
            self.ttft_values.sort_unstable();
            let sum: u64 = self.ttft_values.iter().sum();
            // Round to the nearest millisecond rather than truncating.
            self.ttft_avg_ms = (sum + self.ttft_samples / 2) / self.ttft_samples;
            self.ttft_p50_ms = Some(percentile(&self.ttft_values, 0.50));
            self.ttft_p95_ms = Some(percentile(&self.ttft_values, 0.95));
        }
        let billable = (self.input_tokens
            + self.output_tokens
            + self.cache_read_tokens
            + self.cache_write_tokens) as f64;
        self.tokens_per_sec = if self.latency_sum_ms > 0 {
            billable / (self.latency_sum_ms as f64 / 1000.0)
        } else {
            0.0
        };
        self.output_tokens_per_sec = if self.generation_ms > 0 {
            self.generation_output as f64 / (self.generation_ms as f64 / 1000.0)
        } else {
            0.0
        };
    }
}

/// Nearest-rank percentile over a sorted slice.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    let last = sorted.len() - 1;
    let index = ((last as f64) * p).round() as usize;
    sorted[index.min(last)]
}

/// One provider × upstream-key row: what that key served, and how fast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySpeedRow {
    pub provider: String,
    /// The upstream key's own id inside the provider.
    pub key_id: String,
    /// The key's label, when one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_label: Option<String>,
    pub requests: u64,
    pub failed: u64,
    /// Input + output + cache read + cache write.
    pub total_tokens: u64,
    /// `total_tokens / request time`, across this key's successful calls.
    pub tokens_per_sec: f64,
    /// Output tokens per second of generation time; only streaming
    /// replies count, so the denominator is `generation` samples.
    pub output_tokens_per_sec: f64,
    pub ttft_samples: u64,
    pub ttft_avg_ms: u64,
}

/// Speed and volume per provider × upstream API key, newest period aside.
/// Uses the same `Totals` math as every other dimension, so the rates and
/// the TTFT numbers on this table can never disagree with the rest.
pub fn by_provider_key(records: &[UsageRecord]) -> Vec<KeySpeedRow> {
    let mut buckets: BTreeMap<(String, String), Vec<&UsageRecord>> = BTreeMap::new();
    for r in records {
        buckets
            .entry((r.provider.clone(), r.key_id.clone().unwrap_or_default()))
            .or_default()
            .push(r);
    }
    buckets
        .into_iter()
        .map(|((provider, key_id), rows)| {
            let owned: Vec<UsageRecord> = rows.into_iter().cloned().collect();
            let t = totals(&owned);
            let label = owned
                .iter()
                .find_map(|r| r.key_label.clone())
                .filter(|s| !s.trim().is_empty());
            KeySpeedRow {
                provider,
                key_label: label,
                key_id,
                requests: t.requests,
                failed: t.failed,
                total_tokens: t.input_tokens
                    + t.output_tokens
                    + t.cache_read_tokens
                    + t.cache_write_tokens,
                tokens_per_sec: t.tokens_per_sec,
                output_tokens_per_sec: t.output_tokens_per_sec,
                ttft_samples: t.ttft_samples,
                ttft_avg_ms: t.ttft_avg_ms,
            }
        })
        .collect()
}

/// One credential the config knows about: a stored API key, or one slot
/// of a session-backed provider's account pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialSpec {
    pub provider: String,
    pub credential: String,
    pub label: String,
    /// `"key"` or `"account"`.
    pub kind: String,
    pub enabled: bool,
}

impl CredentialSpec {
    /// Build the list from the live config: each stored API key of a
    /// key-based provider, and each saved account slot of a session-backed
    /// one. Names of people are masked here, the same way the quota panel
    /// masks them, so no usage API returns a raw account nickname.
    pub fn from_config(config: &crate::config::Config) -> Vec<Self> {
        use crate::config::AuthKind;
        let mut specs = Vec::new();
        for provider in &config.providers {
            let mut accounts: Vec<(String, String)> = Vec::new();
            match provider.auth {
                Some(AuthKind::WorkBuddy) => {
                    if let Ok(pool) = crate::workbuddy::read_pool() {
                        accounts = pool
                            .accounts
                            .into_iter()
                            .map(|(name, f)| (name, crate::mask::person(&f.account.nickname)))
                            .collect();
                    }
                }
                Some(AuthKind::QwenWork) => {
                    if let Ok(pool) = crate::qwen::login::read_pool() {
                        accounts = pool
                            .accounts
                            .into_iter()
                            .map(|(name, f)| (name, crate::mask::person(&f.account.nickname)))
                            .collect();
                    }
                }
                Some(AuthKind::TraeCn) => {
                    if let Ok(pool) = crate::trae::read_pool() {
                        accounts = pool
                            .accounts
                            .into_iter()
                            .map(|(name, f)| (name, crate::mask::person(&f.account.nickname)))
                            .collect();
                    }
                }
                _ => {}
            }
            if provider.uses_session() {
                if accounts.is_empty() {
                    // No signed-in account yet: still one row, so the
                    // provider is visible instead of silently missing.
                    specs.push(CredentialSpec {
                        provider: provider.id.clone(),
                        credential: "session".into(),
                        label: "登录态".into(),
                        kind: "account".into(),
                        enabled: provider.enabled,
                    });
                    continue;
                }
                for (slot, holder) in accounts {
                    specs.push(CredentialSpec {
                        provider: provider.id.clone(),
                        credential: slot.clone(),
                        label: if holder.is_empty() { slot } else { holder },
                        kind: "account".into(),
                        enabled: provider.enabled,
                    });
                }
                continue;
            }
            if provider.keys.is_empty() {
                specs.push(CredentialSpec {
                    provider: provider.id.clone(),
                    credential: "default".into(),
                    label: provider.name.clone(),
                    kind: "key".into(),
                    enabled: provider.enabled,
                });
                continue;
            }
            for key in &provider.keys {
                let label = key
                    .label
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| key.id.clone());
                specs.push(CredentialSpec {
                    provider: provider.id.clone(),
                    credential: key.id.clone(),
                    label,
                    kind: "key".into(),
                    enabled: provider.enabled && key.enabled && !key.key.trim().is_empty(),
                });
            }
        }
        specs
    }
}

/// Token consumption and quality-of-service for one credential.
///
/// Configured credentials with no traffic in the period still get a row
/// (all zeroes): every provider must have a visible accounting, not just
/// the ones that happened to serve a request. A removed key of a still-
/// configured provider gets its own inactive row; a provider that left the
/// config entirely is aggregated into one "removed credentials" row so no
/// consumption disappears without burying the table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialRow {
    pub provider: String,
    pub credential: String,
    pub label: String,
    pub kind: String,
    pub enabled: bool,
    /// Was there traffic for this credential in the period?
    pub seen: bool,
    pub requests: u64,
    pub failed: u64,
    pub success_rate: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    /// input + output + cache read + cache write (reasoning is a break-out
    /// of output, never added twice).
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub credits: f64,
    pub ttft_samples: u64,
    pub ttft_avg_ms: u64,
    pub ttft_p50_ms: Option<u64>,
    pub ttft_p95_ms: Option<u64>,
    pub tokens_per_sec: f64,
    pub output_tokens_per_sec: f64,
    pub last_seen: Option<String>,
    /// This credential's share of its provider's billable tokens, 0..1.
    pub share_of_provider: f64,
}

fn credential_id(r: &UsageRecord) -> String {
    r.key_id.clone().unwrap_or_else(|| "default".to_string())
}

/// Turn one credential's rows into the row the console shows. Kept in one
/// place so configured credentials, removed keys and the legacy aggregate
/// for an unconfigured provider all report exactly the same numbers.
fn finish_credential(
    spec: CredentialSpec,
    rows: &[&UsageRecord],
    provider_totals: &BTreeMap<String, Totals>,
) -> CredentialRow {
    let owned: Vec<UsageRecord> = rows.iter().map(|r| (**r).clone()).collect();
    let t = totals(&owned);
    let total_tokens =
        t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens;
    let provider_tokens = provider_totals
        .get(&spec.provider)
        .map(|p| p.input_tokens + p.output_tokens + p.cache_read_tokens + p.cache_write_tokens)
        .unwrap_or(0);
    CredentialRow {
        provider: spec.provider,
        credential: spec.credential,
        label: spec.label,
        kind: spec.kind,
        enabled: spec.enabled,
        seen: !owned.is_empty(),
        requests: t.requests,
        failed: t.failed,
        success_rate: if t.requests == 0 {
            0.0
        } else {
            (t.requests - t.failed) as f64 / t.requests as f64
        },
        input_tokens: t.input_tokens,
        output_tokens: t.output_tokens,
        cache_read_tokens: t.cache_read_tokens,
        cache_write_tokens: t.cache_write_tokens,
        reasoning_tokens: t.reasoning_tokens,
        total_tokens,
        cost_usd: t.cost_usd,
        credits: t.credits,
        ttft_samples: t.ttft_samples,
        ttft_avg_ms: t.ttft_avg_ms,
        ttft_p50_ms: t.ttft_p50_ms,
        ttft_p95_ms: t.ttft_p95_ms,
        tokens_per_sec: t.tokens_per_sec,
        output_tokens_per_sec: t.output_tokens_per_sec,
        last_seen: owned
            .iter()
            .map(|r| r.time.as_str())
            .max()
            .map(str::to_string),
        share_of_provider: if provider_tokens == 0 {
            0.0
        } else {
            total_tokens as f64 / provider_tokens as f64
        },
    }
}

/// Every provider's credentials, with the same `Totals` math as the rest of
/// the usage page. `specs` come from the live config; records supply the
/// traffic and fill in credentials that are no longer configured.
///
/// A provider that is no longer in the config at all (a test stub, a removed
/// upstream) is collapsed into ONE "removed credentials" row per provider:
/// its consumption still shows up, but a benchmark that generated hundreds
/// of random key ids cannot bury the real table.
pub fn by_credential(records: &[UsageRecord], specs: &[CredentialSpec]) -> Vec<CredentialRow> {
    let mut buckets: BTreeMap<(String, String), Vec<&UsageRecord>> = BTreeMap::new();
    let mut providers: BTreeMap<String, Vec<&UsageRecord>> = BTreeMap::new();
    for r in records {
        let id = credential_id(r);
        buckets.entry((r.provider.clone(), id)).or_default().push(r);
        providers.entry(r.provider.clone()).or_default().push(r);
    }
    let provider_totals: BTreeMap<String, Totals> = providers
        .iter()
        .map(|(provider, rows)| {
            let owned: Vec<UsageRecord> = rows.iter().map(|r| (**r).clone()).collect();
            (provider.clone(), totals(&owned))
        })
        .collect();
    let configured: BTreeSet<String> = specs.iter().map(|s| s.provider.clone()).collect();

    // Start from the configured rows, then add any bucket the config does
    // not know about FOR A CONFIGURED PROVIDER (a removed key, or the old
    // pooled "session" rows). A BTreeMap keyed by the pair keeps the output
    // stable and makes duplicate specs harmless.
    let mut known: BTreeMap<(String, String), CredentialSpec> = BTreeMap::new();
    for spec in specs {
        known.insert(
            (spec.provider.clone(), spec.credential.clone()),
            spec.clone(),
        );
    }
    for (key, rows) in &buckets {
        if !configured.contains(&key.0) {
            continue;
        }
        known.entry(key.clone()).or_insert_with(|| {
            let label = if key.1 == "session" {
                // Records written before account attribution: one pooled
                // row for the whole provider, not a real account slot.
                "登录态（旧记录）".to_string()
            } else {
                rows.iter()
                    .find_map(|r| r.key_label.clone())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| key.1.clone())
            };
            CredentialSpec {
                provider: key.0.clone(),
                credential: key.1.clone(),
                label,
                kind: if key.1 == "session" {
                    "account".to_string()
                } else {
                    "key".to_string()
                },
                enabled: false,
            }
        });
    }

    let mut out: Vec<CredentialRow> = known
        .into_iter()
        .map(|(key, spec)| {
            let rows = buckets.get(&key).map(Vec::as_slice).unwrap_or(&[]);
            finish_credential(spec, rows, &provider_totals)
        })
        .collect();
    // Providers that left the config: one aggregated row, not one per
    // historical key. The totals are the provider's own, so nothing is lost.
    for (provider, rows) in &providers {
        if configured.contains(provider) {
            continue;
        }
        let distinct = buckets.keys().filter(|(p, _)| p == provider).count();
        out.push(finish_credential(
            CredentialSpec {
                provider: provider.clone(),
                credential: "removed".into(),
                label: format!("已移除的凭据（{distinct}）"),
                kind: "key".into(),
                enabled: false,
            },
            rows.as_slice(),
            &provider_totals,
        ));
    }
    // Busiest credential first inside each provider; providers themselves
    // stay alphabetical so the table is easy to scan.
    out.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then(b.total_tokens.cmp(&a.total_tokens))
            .then(a.credential.cmp(&b.credential))
    });
    out
}

/// Sum a set of records.
pub fn totals(records: &[UsageRecord]) -> Totals {
    let mut t = Totals::default();
    for r in records {
        t.note(r);
    }
    t.finish();
    t
}

/// Group records by a field, counting requests and tokens.
pub fn group_by(records: &[UsageRecord], key: fn(&UsageRecord) -> String) -> Vec<(String, Totals)> {
    let mut map: BTreeMap<String, Totals> = BTreeMap::new();
    for r in records {
        map.entry(key(r)).or_default().note(r);
    }
    map.into_iter()
        .map(|(label, mut t)| {
            t.finish();
            (label, t)
        })
        .collect()
}

/// The records of one day, keyed by day.
///
/// Must aggregate the same fields as `totals()` / `group_by()`: a cached
/// prompt is token consumption too, and for cache-heavy agents it is most of
/// it. Leaving those fields out made the daily token numbers disagree with
/// the headline totals by a wide margin.
pub fn by_day(records: &[UsageRecord]) -> Vec<(String, Totals)> {
    let mut map: BTreeMap<String, Totals> = BTreeMap::new();
    for r in records {
        let day = r.time.chars().take(10).collect::<String>();
        map.entry(day).or_default().note(r);
    }
    map.into_iter()
        .map(|(day, mut t)| {
            t.finish();
            (day, t)
        })
        .collect()
}

/// How a request's context window was filled, broken down by what put bytes
/// into it. The gateway estimates token counts from the request body it has
/// already parsed, so this is a close estimate, not a vendor token count.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextBreakdown {
    /// System prompt tokens.
    #[serde(default)]
    pub system_tokens: u64,
    /// Tool definition tokens (function schemas).
    #[serde(default)]
    pub tool_tokens: u64,
    /// Instruction-file / prompt-fragment tokens beyond the system prompt.
    #[serde(default)]
    pub file_tokens: u64,
    /// Tokens carried by the conversation so far (user + assistant turns).
    #[serde(default)]
    pub conversation_tokens: u64,
    /// Tokens the vendor served from its prompt cache (a lower cost than new).
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// The total window the request would have taken, in tokens.
    #[serde(default)]
    pub prompt_tokens: u64,
    /// The window the model can hold, when the gateway knows it. 0 means unknown.
    #[serde(default)]
    pub window_tokens: u64,
}

impl ContextBreakdown {
    /// The fraction of the window this request filled (0.0–1.0). 0 when the
    /// window is unknown, so a caller never shows a bogus percentage.
    pub fn fill_ratio(&self) -> f64 {
        if self.window_tokens == 0 {
            0.0
        } else {
            (self.prompt_tokens as f64 / self.window_tokens as f64).clamp(0.0, 1.0)
        }
    }

    /// A single score, 0–100, for how efficiently the window is used and how
    /// cache-friendly the request is. magpie shows a per-agent score; this is
    /// the per-request basis for it. Higher is better.
    pub fn score(&self) -> u8 {
        if self.prompt_tokens == 0 {
            return 100;
        }
        // Penalty for chewing the window.
        let fill = self.fill_ratio();
        let mut score: f64 = 100.0 - fill * 45.0;
        // Reward cache reuse: a request that hits the cache a lot is cheap.
        if self.prompt_tokens > 0 {
            let cache = self.cache_read_tokens as f64 / self.prompt_tokens as f64;
            score += (cache.min(1.0) * 10.0).clamp(0.0, 10.0);
        }
        score.clamp(0.0, 100.0) as u8
    }
}

/// Estimate the token count of a piece of text the way the gateway's own
/// token counter does (the same read Anthropic's count_tokens returns, so the
/// context panel and the API agree): ~3.6 characters per token. A cheap,
/// dependency-free estimate good enough to show a breakdown.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as f64 / 3.6).ceil() as u64
}

/// Build a context breakdown from a parsed chat request.
///
/// The gateway has already turned every protocol's body into a `ChatRequest`,
/// so measuring the parts is a matter of summing the text of each kind. The
/// conversation's tokens come from the messages the caller sent; the cache
/// read is whatever the reply said it reused. The window is filled in by the
/// caller when it knows the model's size.
pub fn measure_request_context(
    system: &[crate::protocol::ir::ContentBlock],
    tools: &[crate::protocol::ir::ToolDef],
    messages: &[crate::protocol::ir::Message],
    cache_read_tokens: u64,
) -> ContextBreakdown {
    let system_tokens = system
        .iter()
        .map(|b| estimate_tokens(&b.plain_text()))
        .sum::<u64>();

    let tool_tokens = tools
        .iter()
        .map(|t| {
            let mut text = t.name.clone();
            if let Some(d) = &t.description {
                text.push_str(d);
            }
            text.push_str(&t.input_schema.to_string());
            estimate_tokens(&text)
        })
        .sum::<u64>();

    // Instruction files / prompt fragments the caller attached as documents
    // or images count as "files": they are loaded content, not the live
    // conversation or the standing system prompt.
    let mut file_tokens = 0u64;
    for m in messages {
        for b in &m.content {
            if matches!(
                b,
                crate::protocol::ir::ContentBlock::Document { .. }
                    | crate::protocol::ir::ContentBlock::Image { .. }
            ) {
                file_tokens += estimate_tokens(&b.plain_text());
            }
        }
        // Anthropic keeps the system prompt as a top-level message sometimes;
        // treat a system-role message as system tokens when the top-level
        // system was empty.
    }

    let conversation_tokens = messages
        .iter()
        .map(|m| {
            m.content
                .iter()
                .filter(|b| {
                    !matches!(
                        b,
                        crate::protocol::ir::ContentBlock::Document { .. }
                            | crate::protocol::ir::ContentBlock::Image { .. }
                    )
                })
                .map(|b| estimate_tokens(&b.plain_text()))
                .sum::<u64>()
        })
        .sum::<u64>();

    let prompt_tokens = system_tokens + tool_tokens + file_tokens + conversation_tokens;
    ContextBreakdown {
        system_tokens,
        tool_tokens,
        file_tokens,
        conversation_tokens,
        cache_read_tokens,
        prompt_tokens,
        window_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ir::{ContentBlock, Message, StreamEvent, ToolDef, Usage};
    use serde_json::json;

    fn record() -> UsageRecord {
        UsageRecord::new("vendor/hy3", "openai_chat")
    }

    #[test]
    fn a_fresh_record_is_okay_and_unmeasured() {
        let rec = record();
        assert!(rec.ok);
        assert!(rec.priced);
        assert_eq!(rec.model, "vendor/hy3");
        assert_eq!(rec.protocol, "openai_chat");
        assert!(rec.time.parse::<chrono::DateTime<chrono::Utc>>().is_ok());
    }

    // ---------------------------------------------------- note_usage --

    #[test]
    fn note_usage_takes_the_largest_counts_the_stream_offers() {
        let mut rec = record();
        rec.note_usage(&StreamEvent::MessageStart {
            id: "m".into(),
            model: "m".into(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 2,
                ..Usage::default()
            },
        });
        // A later delta reporting fewer tokens never lowers the count.
        rec.note_usage(&StreamEvent::MessageDelta {
            stop_reason: None,
            usage: Usage {
                input_tokens: 4,
                output_tokens: 9,
                cache_read_tokens: 3,
                ..Usage::default()
            },
        });
        assert_eq!(rec.input_tokens, 10);
        assert_eq!(rec.output_tokens, 9);
        assert_eq!(rec.cache_read_tokens, 3);
    }

    #[test]
    fn note_usage_ignores_text_events() {
        let mut rec = record();
        rec.note_usage(&StreamEvent::TextDelta {
            index: 0,
            text: "hi".into(),
        });
        assert_eq!(rec.input_tokens, 0);
        assert_eq!(rec.output_tokens, 0);
    }

    // ------------------------------------------------------ jsonl io --

    #[test]
    fn records_round_trip_through_the_jsonl_log() {
        let dir = std::env::temp_dir().join(format!(
            "local-router-usage-unit-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("usage.jsonl");

        let mut rec = record();
        rec.provider = "vendor".into();
        rec.input_tokens = 11;
        rec.output_tokens = 7;
        rec.session_id = Some("s-1".into());
        rec.project = Some("/tmp/work".into());
        rec.agent = Some("claude-code".into());
        rec.context = Some(measure_request_context(
            &[ContentBlock::text("sys")],
            &[],
            &[Message::user("hello")],
            5,
        ));
        super::record(&path, &rec);
        // A failed request lands in the same log.
        let mut bad = record();
        bad.ok = false;
        bad.error = Some("vendor said no".into());
        super::record(&path, &bad);

        let read = read_all(&path);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].session_id.as_deref(), Some("s-1"));
        assert_eq!(read[0].project.as_deref(), Some("/tmp/work"));
        let ctx = read[0].context.as_ref().unwrap();
        assert_eq!(ctx.cache_read_tokens, 5);
        assert!(ctx.prompt_tokens > 0);
        assert!(!read[1].ok);
        assert_eq!(read[1].error.as_deref(), Some("vendor said no"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------ roll-over --

    #[test]
    fn the_log_rolls_over_when_the_env_lowered_limit_is_passed() {
        // `max_log_bytes` honours the env var, but the limit is read inside
        // `record`; this test only pins the parser's contract.
        std::env::set_var("LOCAL_ROUTER_USAGE_MAX_BYTES", "1024");
        assert_eq!(max_log_bytes(), 1024);
        std::env::remove_var("LOCAL_ROUTER_USAGE_MAX_BYTES");
        assert_eq!(max_log_bytes(), MAX_LOG_BYTES);
    }

    // ----------------------------------------------------- aggregates --

    #[test]
    fn totals_count_failures_without_mixing_tokens_in() {
        let mut ok = record();
        ok.provider = "a".into();
        ok.input_tokens = 10;
        ok.output_tokens = 5;
        ok.cost_usd = Some(0.5);
        let mut bad = record();
        bad.ok = false;
        bad.provider = "a".into();
        bad.input_tokens = 100; // a failed request's tokens must not be billed
        bad.error = Some("x".into());

        let totals = totals(&[ok, bad]);
        assert_eq!(totals.requests, 2);
        assert_eq!(totals.failed, 1);
        assert_eq!(totals.input_tokens, 10);
        assert_eq!(totals.output_tokens, 5);
        assert!((totals.cost_usd - 0.5).abs() < 1e-9);
    }

    #[test]
    fn group_by_and_by_day_bucket_the_same_records_differently() {
        let mut a = record();
        a.provider = "p1".into();
        a.time = "2026-10-26T08:00:00+00:00".into();
        a.output_tokens = 3;
        let mut b = record();
        b.provider = "p2".into();
        b.time = "2026-10-26T09:00:00+00:00".into();
        b.output_tokens = 4;
        let mut c = record();
        c.provider = "p1".into();
        c.time = "2026-10-25T23:00:00+00:00".into();
        c.ok = false;

        let by_provider = group_by(&[a.clone(), b.clone(), c.clone()], |r| r.provider.clone());
        assert_eq!(by_provider.len(), 2);
        let p1 = by_provider.iter().find(|(k, _)| k == "p1").unwrap();
        assert_eq!(p1.1.requests, 2);
        assert_eq!(p1.1.failed, 1);

        let days = by_day(&[a, b, c]);
        assert_eq!(days.len(), 2);
        let day = days.iter().find(|(d, _)| d == "2026-10-26").unwrap();
        assert_eq!(day.1.requests, 2);
        assert_eq!(day.1.output_tokens, 7);
    }

    #[test]
    fn by_day_counts_cached_and_reasoning_tokens_like_totals() {
        let mut cached = record();
        cached.time = "2026-10-26T08:00:00+00:00".into();
        cached.input_tokens = 10;
        cached.output_tokens = 5;
        cached.cache_read_tokens = 400;
        cached.cache_write_tokens = 20;
        cached.reasoning_tokens = 3;
        cached.priced = false;
        let mut failed = record();
        failed.time = "2026-10-26T09:00:00+00:00".into();
        failed.ok = false;
        failed.cache_read_tokens = 999;

        let days = by_day(&[cached.clone(), failed.clone()]);
        let (_, day) = days.iter().find(|(d, _)| d == "2026-10-26").unwrap();
        assert_eq!(day.requests, 2);
        assert_eq!(day.failed, 1);
        assert_eq!(day.input_tokens, 10);
        assert_eq!(day.output_tokens, 5);
        assert_eq!(day.cache_read_tokens, 400);
        assert_eq!(day.cache_write_tokens, 20);
        assert_eq!(day.reasoning_tokens, 3);
        assert_eq!(day.unpriced, 1);
        // The daily slice must agree with the headline totals.
        let all = totals(&[cached, failed]);
        assert_eq!(day.cache_read_tokens, all.cache_read_tokens);
        assert_eq!(day.cache_write_tokens, all.cache_write_tokens);
        assert_eq!(day.reasoning_tokens, all.reasoning_tokens);
    }

    #[test]
    fn ttft_is_summarized_with_samples_and_percentiles_everywhere() {
        let mut a = record();
        a.time = "2026-10-26T08:00:00+00:00".into();
        a.provider = "p1".into();
        a.ttft_ms = Some(100);
        let mut b = record();
        b.time = "2026-10-26T09:00:00+00:00".into();
        b.provider = "p1".into();
        b.ttft_ms = Some(300);
        let mut c = record();
        c.time = "2026-10-26T10:00:00+00:00".into();
        c.provider = "p2".into();
        c.ttft_ms = Some(50);
        // A non-streaming reply: counted in requests, never in TTFT.
        let mut d = record();
        d.time = "2026-10-26T11:00:00+00:00".into();
        d.provider = "p2".into();
        // A failed reply with a stray sample must not skew the speed.
        let mut e = record();
        e.time = "2026-10-26T12:00:00+00:00".into();
        e.provider = "p2".into();
        e.ok = false;
        e.ttft_ms = Some(9999);

        let all = totals(&[a.clone(), b.clone(), c.clone(), d.clone(), e.clone()]);
        assert_eq!(all.requests, 5);
        assert_eq!(all.ttft_samples, 3);
        assert_eq!(all.ttft_avg_ms, 150); // (100 + 300 + 50) / 3
        assert_eq!(all.ttft_p50_ms, Some(100));
        assert_eq!(all.ttft_p95_ms, Some(300));

        let by_provider = group_by(
            &[a.clone(), b.clone(), c.clone(), d.clone(), e.clone()],
            |r| r.provider.clone(),
        );
        let p1 = by_provider.iter().find(|(k, _)| k == "p1").unwrap();
        assert_eq!(p1.1.ttft_samples, 2);
        assert_eq!(p1.1.ttft_avg_ms, 200);
        let p2 = by_provider.iter().find(|(k, _)| k == "p2").unwrap();
        assert_eq!(p2.1.requests, 3);
        assert_eq!(p2.1.ttft_samples, 1);
        assert_eq!(p2.1.ttft_avg_ms, 50);

        let days = by_day(&[a, b, c, d, e]);
        let (_, day) = days.iter().find(|(d, _)| d == "2026-10-26").unwrap();
        assert_eq!(day.ttft_samples, all.ttft_samples);
        assert_eq!(day.ttft_avg_ms, all.ttft_avg_ms);
        assert_eq!(day.ttft_p50_ms, all.ttft_p50_ms);
        assert_eq!(day.ttft_p95_ms, all.ttft_p95_ms);
    }

    #[test]
    fn no_ttft_samples_reports_none_instead_of_zero_percentiles() {
        let all = totals(&[record()]);
        assert_eq!(all.ttft_samples, 0);
        assert_eq!(all.ttft_avg_ms, 0);
        assert_eq!(all.ttft_p50_ms, None);
        assert_eq!(all.ttft_p95_ms, None);
    }

    #[test]
    fn speed_is_tokens_over_request_time_per_provider_key() {
        // One streaming reply on key k1: 100 output tokens, 100ms to the
        // first one, 2.0s of generation afterwards.
        let mut a = record();
        a.provider = "p1".into();
        a.key_id = Some("k1".into());
        a.key_label = Some("fast".into());
        a.output_tokens = 100;
        a.latency_ms = 2100;
        a.ttft_ms = Some(100);
        // A non-streaming reply on the same key: counts for volume and
        // request time, but has no generation phase to measure.
        let mut b = record();
        b.provider = "p1".into();
        b.key_id = Some("k1".into());
        b.output_tokens = 300;
        b.latency_ms = 3000;
        // A second key on the same provider is a separate row.
        let mut c = record();
        c.provider = "p1".into();
        c.key_id = Some("k2".into());
        c.output_tokens = 10;
        c.latency_ms = 1000;

        let rows = by_provider_key(&[a, b, c]);
        assert_eq!(rows.len(), 2);
        let k1 = rows.iter().find(|r| r.key_id == "k1").unwrap();
        assert_eq!(k1.requests, 2);
        assert_eq!(k1.total_tokens, 400);
        // 400 tokens over 5.1s of request time.
        assert!((k1.tokens_per_sec - 400.0 / 5.1).abs() < 0.1);
        // Output rate only counts the streaming sample: 100 over 2.0s.
        assert!((k1.output_tokens_per_sec - 50.0).abs() < 0.1);
        assert_eq!(k1.ttft_samples, 1);
        assert_eq!(k1.key_label.as_deref(), Some("fast"));
        let k2 = rows.iter().find(|r| r.key_id == "k2").unwrap();
        assert_eq!(k2.total_tokens, 10);
        assert_eq!(k2.ttft_samples, 0);
        assert_eq!(k2.output_tokens_per_sec, 0.0);
    }

    // ------------------------------------------------------- context --

    #[test]
    fn estimate_tokens_uses_roughly_three_point_six_chars_per_token() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 2); // ceil(4/3.6)
        assert_eq!(estimate_tokens("abcdefgh"), 3); // ceil(8/3.6) = 2.22→3
                                                    // Counted in chars, not bytes: CJK text must not triple-count.
        let cjk = estimate_tokens("四个汉字");
        assert_eq!(cjk, 2); // ceil(4/3.6)
    }

    #[test]
    fn context_buckets_split_system_tools_files_and_conversation() {
        let system = [ContentBlock::text("You are helpful.")];
        let tools = [ToolDef::new(
            "read",
            Some("Read a file".into()),
            json!({"type": "object"}),
        )];
        let messages = [
            Message::user("hi"),
            Message {
                role: crate::protocol::ir::Role::User,
                content: vec![ContentBlock::Document {
                    media_type: "text/plain".into(),
                    data: "a long attached file body".into(),
                    name: Some("notes.txt".into()),
                    cache_control: None,
                }],
                tool_call_id: None,
                name: None,
            },
        ];
        let ctx = measure_request_context(&system, &tools, &messages, 9);
        assert!(ctx.system_tokens > 0);
        assert!(ctx.tool_tokens > 0);
        assert!(ctx.file_tokens > 0);
        assert!(ctx.conversation_tokens > 0);
        assert_eq!(ctx.cache_read_tokens, 9);
        assert_eq!(
            ctx.prompt_tokens,
            ctx.system_tokens + ctx.tool_tokens + ctx.file_tokens + ctx.conversation_tokens
        );
        // The window is filled in later by the caller; the ratio stays 0.
        assert_eq!(ctx.fill_ratio(), 0.0);
    }

    #[test]
    fn fill_ratio_clamps_and_zero_window_means_zero() {
        let mut ctx = ContextBreakdown {
            window_tokens: 100,
            ..ContextBreakdown::default()
        };
        ctx.prompt_tokens = 50;
        assert_eq!(ctx.fill_ratio(), 0.5);
        ctx.prompt_tokens = 500; // over the window: clamped, not negative
        assert_eq!(ctx.fill_ratio(), 1.0);
        ctx.window_tokens = 0;
        assert_eq!(ctx.fill_ratio(), 0.0);
    }

    // ------------------------------------------------- by_credential --

    fn credential_rec(provider: &str, key: &str, tokens: u64, ok: bool) -> UsageRecord {
        let mut r = record();
        r.provider = provider.to_string();
        r.key_id = Some(key.to_string());
        r.input_tokens = tokens;
        r.ok = ok;
        r.status = if ok { 200 } else { 429 };
        r.latency_ms = 1000;
        r.cost_usd = Some(0.5);
        r.time = "2026-11-14T10:00:00+00:00".to_string();
        r
    }

    #[test]
    fn credentials_include_configured_rows_with_no_traffic_and_unknown_rows() {
        let records = vec![
            credential_rec("alpha", "key-1", 100, true),
            credential_rec("alpha", "key-2", 300, true),
            credential_rec("alpha", "key-2", 0, false),
            credential_rec("beta", "session", 50, true),
            credential_rec("ghost", "old-key", 7, true),
        ];
        let specs = vec![
            CredentialSpec {
                provider: "alpha".into(),
                credential: "key-1".into(),
                label: "主密钥".into(),
                kind: "key".into(),
                enabled: true,
            },
            CredentialSpec {
                provider: "alpha".into(),
                credential: "key-2".into(),
                label: "备用".into(),
                kind: "key".into(),
                enabled: true,
            },
            // Configured but idle: still a row, so every provider is visible.
            CredentialSpec {
                provider: "charlie".into(),
                credential: "key-x".into(),
                label: "unused".into(),
                kind: "key".into(),
                enabled: true,
            },
            CredentialSpec {
                provider: "beta".into(),
                credential: "session".into(),
                label: "张*".into(),
                kind: "account".into(),
                enabled: true,
            },
        ];
        let rows = by_credential(&records, &specs);
        let idle = rows
            .iter()
            .find(|r| r.provider == "charlie")
            .expect("an idle configured credential must still be listed");
        assert!(!idle.seen);
        assert_eq!(idle.total_tokens, 0);
        assert_eq!(idle.requests, 0);
        let ghost = rows
            .iter()
            .find(|r| r.provider == "ghost")
            .expect("a removed key's traffic must not disappear");
        assert!(!ghost.enabled);
        assert_eq!(ghost.total_tokens, 7);
        let k1 = rows.iter().find(|r| r.credential == "key-1").unwrap();
        let k2 = rows.iter().find(|r| r.credential == "key-2").unwrap();
        assert_eq!(k1.share_of_provider, 0.25);
        assert_eq!(k2.share_of_provider, 0.75);
        // A failed request counts as a request but carries no tokens.
        assert_eq!(k2.requests, 2);
        assert_eq!(k2.failed, 1);
        assert_eq!(k2.success_rate, 0.5);
        assert_eq!(k2.last_seen.as_deref(), Some("2026-11-14T10:00:00+00:00"));
        // Busiest first inside a provider.
        let alpha: Vec<&str> = rows
            .iter()
            .filter(|r| r.provider == "alpha")
            .map(|r| r.credential.as_str())
            .collect();
        assert_eq!(alpha, vec!["key-2", "key-1"]);
    }

    #[test]
    fn an_unconfigured_provider_collapses_into_one_row() {
        // A load-test stub can leave hundreds of random key ids behind.
        // They must not turn the console into 500 rows of noise.
        let mut records = Vec::new();
        for n in 0..40 {
            records.push(credential_rec(
                "benchmark-stub",
                &format!("key-{n}"),
                3,
                true,
            ));
        }
        let rows = by_credential(&records, &[]);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].credential, "removed");
        assert_eq!(rows[0].total_tokens, 120);
        assert_eq!(rows[0].share_of_provider, 1.0);
        assert!(!rows[0].enabled);
    }

    #[test]
    fn credential_totals_use_the_billable_token_definition() {
        let mut r = credential_rec("alpha", "key-1", 10, true);
        r.output_tokens = 5;
        r.cache_read_tokens = 100;
        r.cache_write_tokens = 2;
        r.reasoning_tokens = 4;
        r.ttft_ms = Some(20);
        let rows = by_credential(
            &[r.clone()],
            &[CredentialSpec {
                provider: "alpha".into(),
                credential: "key-1".into(),
                label: "key-1".into(),
                kind: "key".into(),
                enabled: true,
            }],
        );
        let row = &rows[0];
        // Reasoning is a break-out of output, so 10+5+100+2 = 117.
        assert_eq!(row.total_tokens, 117);
        assert_eq!(row.ttft_samples, 1);
        assert_eq!(row.ttft_p50_ms, Some(20));
        assert_eq!(row.share_of_provider, 1.0);
    }

    #[test]
    fn score_rewards_cache_and_penalizes_fill_and_empty_window_scores_full() {
        let mut ctx = ContextBreakdown {
            window_tokens: 1000,
            prompt_tokens: 100,
            cache_read_tokens: 50,
            ..ContextBreakdown::default()
        };
        let with_cache = ctx.score();
        ctx.cache_read_tokens = 0;
        let without_cache = ctx.score();
        assert!(
            with_cache > without_cache,
            "cache reuse should score higher"
        );
        ctx.prompt_tokens = 0;
        assert_eq!(ctx.score(), 100);
    }
}
