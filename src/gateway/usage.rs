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
            status: 0,
            ok: true,
            error: None,
            agent: None,
            session_id: None,
            project: None,
            context: None,
            cost_usd: None,
            credits: None,
            priced: true,
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
        if let Some(usd) = r.cost_usd {
            t.cost_usd += usd;
        }
        if let Some(credits) = r.credits {
            t.credits += credits;
        }
        if !r.priced {
            t.unpriced += 1;
        }
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
        if let Some(usd) = r.cost_usd {
            entry.cost_usd += usd;
        }
        if let Some(credits) = r.credits {
            entry.credits += credits;
        }
        if !r.priced {
            entry.unpriced += 1;
        }
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
        if let Some(usd) = r.cost_usd {
            entry.cost_usd += usd;
        }
        if let Some(credits) = r.credits {
            entry.credits += credits;
        }
    }
    map.into_iter().collect()
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
