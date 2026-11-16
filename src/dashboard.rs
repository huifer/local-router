//! The terminal dashboard: the same numbers the web console shows, drawn
//! with box characters in the terminal.
//!
//! One render per tick, ANSI-colored, no third-party UI crate. `q` or
//! ctrl-c leaves.

use std::io::Write;
use std::time::Duration;

use crate::gateway::usage::UsageRecord;
use crate::{config, gateway, store};

/// The dashboard refresh cadence.
const TICK: Duration = Duration::from_secs(2);

/// How far back the dashboard looks.
const WINDOW_DAYS: i64 = 7;

pub async fn run() -> anyhow::Result<()> {
    // A terminal is the only place this works.
    if !is_tty() {
        anyhow::bail!("the dashboard needs a terminal; use `local-router usage` instead");
    }

    let sqlite = store::Store::shared();
    let mut out = std::io::stdout();
    let mut tick: u64 = 0;

    loop {
        let snapshot = collect(&sqlite);
        render(&mut out, &snapshot, tick)?;
        tick += 1;

        // Leave on `q` or ctrl-c, without leaving escape codes behind.
        tokio::select! {
            _ = tokio::time::sleep(TICK) => {}
            _ = wait_for_quit() => break,
        }
    }

    let _ = write!(out, "\x1b[2J\x1b[H\x1b[?25h");
    let _ = out.flush();
    Ok(())
}

/// Whether stdout is an interactive terminal.
fn is_tty() -> bool {
    // `test -t 1` is the portable way without adding a crate.
    std::process::Command::new("test")
        .arg("-t")
        .arg("1")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A parked task that resolves when the user presses `q` or ctrl-c.
async fn wait_for_quit() {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        std::future::pending::<()>().await;
    }
    // Read raw bytes until `q` or ^C shows up.
    // The exit status does not matter: any outcome ends the wait, and the
    // loop keeps running only when the key was not `q`/ctrl-c.
    let _child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("read -r -n 1 key; case \"$key\" in q|Q) exit 0;; *) exit 1;; esac")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::inherit())
        .status()
        .await;
    // ctrl-c or another key: stop as well. Falling out of this future
    // ends the select, so the loop breaks either way.
}

/// Everything one frame shows, gathered without holding any lock long.
struct Snapshot {
    version: String,
    gateway_addr: String,
    web_addr: String,
    providers: Vec<ProviderRow>,
    totals: Totals,
    by_day: Vec<(String, DayRow)>,
    by_provider: Vec<(String, DayRow)>,
    recent: Vec<UsageRecord>,
    storage: &'static str,
}

struct ProviderRow {
    id: String,
    enabled: bool,
    models: usize,
    auth: &'static str,
}

#[derive(Default)]
struct Totals {
    requests: i64,
    failed: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_usd: f64,
    credits: f64,
    ttft_samples: i64,
    ttft_avg_ms: i64,
}

#[derive(Default)]
struct DayRow {
    requests: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_usd: f64,
    ttft_samples: i64,
    ttft_avg_ms: i64,
}

/// The billable volume, the same four fields the Usage page adds up.
fn billable(row: &DayRow) -> i64 {
    row.input_tokens + row.output_tokens + row.cache_read_tokens + row.cache_write_tokens
}

/// `avg ms / n samples`, or a dash when nothing streamed in this bucket.
fn ttft_label(samples: i64, avg_ms: i64) -> String {
    if samples > 0 {
        format!("{avg_ms}ms/n{samples}")
    } else {
        "—".to_string()
    }
}

fn from_aggregate(row: &store::AggregateRow) -> DayRow {
    DayRow {
        requests: row.requests,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cache_read_tokens: row.cache_read_tokens,
        cache_write_tokens: row.cache_write_tokens,
        cost_usd: row.cost_usd.unwrap_or_default(),
        ttft_samples: row.ttft_samples,
        ttft_avg_ms: row.ttft_avg_ms,
    }
}

fn from_totals(t: &gateway::usage::Totals) -> DayRow {
    DayRow {
        requests: t.requests as i64,
        input_tokens: t.input_tokens as i64,
        output_tokens: t.output_tokens as i64,
        cache_read_tokens: t.cache_read_tokens as i64,
        cache_write_tokens: t.cache_write_tokens as i64,
        cost_usd: t.cost_usd,
        ttft_samples: t.ttft_samples as i64,
        ttft_avg_ms: t.ttft_avg_ms as i64,
    }
}

fn collect(sqlite: &Option<store::Store>) -> Snapshot {
    let config = config::Store::new().load().unwrap_or_default();
    let since = (chrono::Utc::now() - chrono::Duration::days(WINDOW_DAYS)).to_rfc3339();

    let providers = config
        .providers
        .iter()
        .map(|p| ProviderRow {
            id: p.id.clone(),
            enabled: p.enabled,
            models: p.models.len() + p.media_models.len(),
            auth: if p.uses_session() { "登录态" } else { "key" },
        })
        .collect();

    let (totals, by_day, by_provider, recent, storage) = if let Some(sqlite) = sqlite {
        let totals_row = sqlite.totals_since(&since);
        let totals = Totals {
            requests: totals_row.as_ref().map(|r| r.requests).unwrap_or(0),
            failed: totals_row.as_ref().map(|r| r.failed).unwrap_or(0),
            input_tokens: totals_row.as_ref().map(|r| r.input_tokens).unwrap_or(0),
            output_tokens: totals_row.as_ref().map(|r| r.output_tokens).unwrap_or(0),
            cache_read_tokens: totals_row
                .as_ref()
                .map(|r| r.cache_read_tokens)
                .unwrap_or(0),
            cache_write_tokens: totals_row
                .as_ref()
                .map(|r| r.cache_write_tokens)
                .unwrap_or(0),
            cost_usd: totals_row
                .as_ref()
                .and_then(|r| r.cost_usd)
                .unwrap_or_default(),
            credits: totals_row
                .as_ref()
                .and_then(|r| r.credits)
                .unwrap_or_default(),
            ttft_samples: totals_row.as_ref().map(|r| r.ttft_samples).unwrap_or(0),
            ttft_avg_ms: totals_row.as_ref().map(|r| r.ttft_avg_ms).unwrap_or(0),
        };
        let by_day = sqlite
            .by_day_since(&since)
            .into_iter()
            .map(|row| (row.day.clone().unwrap_or_default(), from_aggregate(&row)))
            .collect();
        let by_provider = sqlite
            .by_provider_since(&since)
            .into_iter()
            .map(|row| (row.label.clone().unwrap_or_default(), from_aggregate(&row)))
            .collect();
        let recent = sqlite.recent(&since, 8);
        (totals, by_day, by_provider, recent, "SQLITE")
    } else {
        let records: Vec<_> = gateway::usage::read_all(&config::Store::new().usage_path())
            .into_iter()
            .filter(|r| r.time.as_str() >= since.as_str())
            .collect();
        // The SQL path counts successful rows only. Keep the JSONL fallback
        // on the same set and let `usage::Totals` do every aggregation, so
        // cache tokens and TTFT cannot drift between the two stores.
        let ok: Vec<_> = records.iter().filter(|r| r.ok).cloned().collect();
        let totals_row = gateway::usage::totals(&ok);
        let totals = Totals {
            requests: totals_row.requests as i64,
            failed: 0,
            input_tokens: totals_row.input_tokens as i64,
            output_tokens: totals_row.output_tokens as i64,
            cache_read_tokens: totals_row.cache_read_tokens as i64,
            cache_write_tokens: totals_row.cache_write_tokens as i64,
            cost_usd: totals_row.cost_usd,
            credits: totals_row.credits,
            ttft_samples: totals_row.ttft_samples as i64,
            ttft_avg_ms: totals_row.ttft_avg_ms as i64,
        };
        let by_day = gateway::usage::by_day(&ok)
            .into_iter()
            .map(|(day, t)| (day, from_totals(&t)))
            .collect();
        let by_provider = gateway::usage::group_by(&ok, |r| r.provider.clone())
            .into_iter()
            .map(|(name, t)| (name, from_totals(&t)))
            .collect();
        let recent = records.iter().rev().take(8).cloned().collect();
        (totals, by_day, by_provider, recent, "JSONL")
    };

    let settings = &config.settings;
    Snapshot {
        version: gateway::client::VERSION.to_string(),
        gateway_addr: format!("{}:{}", settings.gateway_addr, settings.gateway_port),
        // The console is on the gateway's port now.
        web_addr: format!("{}:{}", settings.gateway_addr, settings.gateway_port),
        providers,
        totals,
        by_day,
        by_provider,
        recent,
        storage,
    }
}

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const RED: &str = "\x1b[31m";

fn render(out: &mut impl Write, s: &Snapshot, tick: u64) -> anyhow::Result<()> {
    let (width, _) = terminal_size();
    let width = width.clamp(60, 160);

    let mut body = String::new();
    let line = |c: char| c.to_string().repeat(width - 2);

    // Header.
    body.push_str("\x1b[H\x1b[2J\x1b[?25l");
    body.push_str(&format!(
        "{GREEN}local-router{RESET} {}  {DIM}dashboard · {:<5} · q 退出{RESET}\n",
        s.version,
        format!("t{tick}")
    ));
    body.push_str(&format!("╭{}╮\n", line('─')));

    // Endpoints.
    body.push_str(&format!(
        "│ {CYAN}gateway{RESET}  http://{:<32} {CYAN}console{RESET} http://{:<34}│\n",
        s.gateway_addr, s.web_addr
    ));
    body.push_str(&format!("├{}┤\n", line('─')));

    // Providers.
    body.push_str(&format!("│ {DIM}PROVIDERS{RESET}\n"));
    if s.providers.is_empty() {
        body.push_str(&format!(
            "│   {DIM}(none — local-router provider add <preset> …){RESET}\n"
        ));
    }
    for p in &s.providers {
        let state = if p.enabled {
            format!("{GREEN}on {RESET}")
        } else {
            format!("{RED}off{RESET}")
        };
        body.push_str(&format!(
            "│   {:<20} {state}  {:<6} {:>3} models\n",
            p.id, p.auth, p.models
        ));
    }
    body.push_str(&format!("├{}┤\n", line('─')));

    // Totals.
    body.push_str(&format!(
        "│ {DIM}WINDOW{RESET}  7d · requests {:>5} · failed {:>3} · tok {:>10} · ttft {}\n",
        s.totals.requests,
        s.totals.failed,
        s.totals.input_tokens
            + s.totals.output_tokens
            + s.totals.cache_read_tokens
            + s.totals.cache_write_tokens,
        ttft_label(s.totals.ttft_samples, s.totals.ttft_avg_ms)
    ));
    body.push_str(&format!(
        "│ {DIM}SPEND {RESET}  ${:<10.4} credits {:<12.1} storage {}\n",
        s.totals.cost_usd, s.totals.credits, s.storage
    ));
    body.push_str(&format!("├{}┤\n", line('─')));

    // Per-provider bars.
    body.push_str(&format!("│ {DIM}BY PLATFORM{RESET}\n"));
    if s.by_provider.is_empty() {
        body.push_str(&format!("│   {DIM}(no traffic yet){RESET}\n"));
    }
    let max_requests = s
        .by_provider
        .iter()
        .map(|(_, r)| r.requests)
        .max()
        .unwrap_or(1)
        .max(1);
    for (name, row) in s.by_provider.iter().take(6) {
        let bar_width = width.saturating_sub(58).max(8);
        let filled = (row.requests as f64 / max_requests as f64 * bar_width as f64) as usize;
        let bar = format!("{GREEN}{}{RESET}", "█".repeat(filled.min(bar_width)));
        body.push_str(&format!(
            "│   {:<16} {:>6} req {:>9} tok ${:<8.3} ttft {:>10} {bar}\n",
            name,
            row.requests,
            billable(row),
            row.cost_usd,
            ttft_label(row.ttft_samples, row.ttft_avg_ms)
        ));
    }
    body.push_str(&format!("├{}┤\n", line('─')));

    // Daily bars.
    body.push_str(&format!("│ {DIM}BY DAY{RESET}\n"));
    let max_day = s
        .by_day
        .iter()
        .map(|(_, r)| r.requests)
        .max()
        .unwrap_or(1)
        .max(1);
    for (day, row) in s.by_day.iter().rev().take(5) {
        let bar_width = width.saturating_sub(58).max(8);
        let filled = (row.requests as f64 / max_day as f64 * bar_width as f64) as usize;
        let bar = format!("{CYAN}{}{RESET}", "▇".repeat(filled.min(bar_width)));
        body.push_str(&format!(
            "│   {day}  {:>5} req {:>9} tok ${:<8.3} {bar}\n",
            row.requests,
            billable(row),
            row.cost_usd
        ));
    }
    body.push_str(&format!("├{}┤\n", line('─')));

    // Recent requests.
    body.push_str(&format!("│ {DIM}RECENT{RESET}\n"));
    if s.recent.is_empty() {
        body.push_str(&format!("│   {DIM}(no requests yet){RESET}\n"));
    }
    for r in s.recent.iter().take(6) {
        let state = if r.ok {
            format!("{GREEN} 200{RESET}")
        } else {
            format!("{RED}{:>3}{RESET}", r.status)
        };
        body.push_str(&format!(
            "│   {} {:<24} {:>6} in {:>7} out ${:<8.4} {}\n",
            r.time.chars().skip(11).take(8).collect::<String>(),
            r.model.chars().take(24).collect::<String>(),
            r.input_tokens,
            r.output_tokens,
            r.cost_usd.unwrap_or(0.0),
            state
        ));
    }
    body.push_str(&format!("╰{}╯\n", line('─')));

    write!(out, "{body}")?;
    out.flush()?;
    Ok(())
}

/// The terminal size, with a sane fallback.
fn terminal_size() -> (usize, usize) {
    // `stty size` needs the terminal; run it in the same process group.
    if let Ok(output) = std::process::Command::new("stty")
        .arg("size")
        .stderr(std::process::Stdio::null())
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut parts = text.split_whitespace();
        if let (Some(rows), Some(cols)) = (parts.next(), parts.next()) {
            if let (Ok(r), Ok(c)) = (rows.parse::<usize>(), cols.parse::<usize>()) {
                return (c, r);
            }
        }
    }
    (80, 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            version: "0.1.0".into(),
            gateway_addr: "127.0.0.1:3435".into(),
            web_addr: "127.0.0.1:3426".into(),
            providers: vec![ProviderRow {
                id: "mock".into(),
                enabled: true,
                models: 2,
                auth: "key",
            }],
            totals: Totals {
                requests: 8,
                failed: 0,
                input_tokens: 77,
                output_tokens: 49,
                cost_usd: 0.25,
                credits: 3.5,
                ttft_samples: 8,
                ttft_avg_ms: 52,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            by_day: vec![(
                "2026-10-26".into(),
                DayRow {
                    requests: 8,
                    input_tokens: 77,
                    output_tokens: 49,
                    cost_usd: 0.25,
                    ..Default::default()
                },
            )],
            by_provider: vec![(
                "mock".into(),
                DayRow {
                    requests: 8,
                    input_tokens: 77,
                    output_tokens: 49,
                    cost_usd: 0.25,
                    ttft_samples: 8,
                    ttft_avg_ms: 52,
                    ..Default::default()
                },
            )],
            recent: Vec::new(),
            storage: "SQLITE",
        }
    }

    #[test]
    fn a_frame_renders_with_the_headline_numbers() {
        let mut out = Vec::new();
        render(&mut out, &snapshot(), 0).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("local-router"));
        assert!(text.contains("0.1.0"));
        assert!(text.contains("mock"));
        assert!(text.contains("8"));
        assert!(text.contains("SQLITE"));
        // The per-provider line and the headline both carry first-token
        // speed with its sample denominator.
        assert!(text.contains("52ms/n8"));
        // Tokens are the billable four-field total, not just in/out.
        assert!(text.contains("126 tok"));
        // The screen is positioned, then the cursor is hidden while drawing.
        assert!(text.starts_with("\x1b[H\x1b[2J"));
    }

    #[test]
    fn an_empty_gateway_renders_the_placeholders() {
        let mut s = snapshot();
        s.providers.clear();
        s.by_provider.clear();
        s.by_day.clear();
        let mut out = Vec::new();
        render(&mut out, &s, 1).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("none —"));
        assert!(text.contains("no traffic yet"));
        // Dividing by a zero max must not panic; bars stay empty.
        assert!(text.contains("t1"));
    }
}
