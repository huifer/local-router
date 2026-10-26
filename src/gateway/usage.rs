//! The record of what each request cost.

use std::collections::BTreeMap;
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
            group: None,
            protocol: protocol.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            latency_ms: 0,
            status: 0,
            ok: true,
            error: None,
            agent: None,
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
}

/// Sum a set of records.
pub fn totals(records: &[UsageRecord]) -> Totals {
    let mut t = Totals::default();
    for r in records {
        t.requests += 1;
        if !r.ok {
            t.failed += 1;
            continue;
        }
        t.input_tokens += r.input_tokens;
        t.output_tokens += r.output_tokens;
        t.cache_read_tokens += r.cache_read_tokens;
        t.cache_write_tokens += r.cache_write_tokens;
        t.reasoning_tokens += r.reasoning_tokens;
    }
    t
}

/// Group records by a field, counting requests and tokens.
pub fn group_by(records: &[UsageRecord], key: fn(&UsageRecord) -> String) -> Vec<(String, Totals)> {
    let mut map: BTreeMap<String, Totals> = BTreeMap::new();
    for r in records {
        let entry = map.entry(key(r)).or_default();
        entry.requests += 1;
        if !r.ok {
            entry.failed += 1;
            continue;
        }
        entry.input_tokens += r.input_tokens;
        entry.output_tokens += r.output_tokens;
        entry.cache_read_tokens += r.cache_read_tokens;
        entry.cache_write_tokens += r.cache_write_tokens;
        entry.reasoning_tokens += r.reasoning_tokens;
    }
    map.into_iter().collect()
}

/// The records of one day, keyed by day.
pub fn by_day(records: &[UsageRecord]) -> Vec<(String, Totals)> {
    let mut map: BTreeMap<String, Totals> = BTreeMap::new();
    for r in records {
        let day = r.time.chars().take(10).collect::<String>();
        let entry = map.entry(day).or_default();
        entry.requests += 1;
        if !r.ok {
            entry.failed += 1;
            continue;
        }
        entry.input_tokens += r.input_tokens;
        entry.output_tokens += r.output_tokens;
    }
    map.into_iter().collect()
}
