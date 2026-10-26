//! The usage log: what it records and how it is kept from growing forever.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use local_router::gateway::usage::{self, UsageRecord};

/// The tests below move the environment around, so they take turns.
fn env_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn record(model: &str) -> UsageRecord {
    let mut rec = UsageRecord::new(model, "openai_chat");
    rec.input_tokens = 10;
    rec.output_tokens = 5;
    rec
}

#[test]
fn a_record_is_written_as_one_line() {
    let _turn = env_guard();
    let dir = std::env::temp_dir().join(format!(
        "local-router-usage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("usage.jsonl");
    usage::record(&path, &record("a/model"));
    usage::record(&path, &record("b/model"));

    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["model"], "a/model");
    assert_eq!(first["input_tokens"], 10);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_log_is_rolled_over_before_it_grows_without_end() {
    let _turn = env_guard();
    let dir = std::env::temp_dir().join(format!(
        "local-router-usage-roll-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("usage.jsonl");

    // A small limit, so the test does not have to write megabytes.
    std::env::set_var("LOCAL_ROUTER_USAGE_MAX_BYTES", "200");
    for index in 0..20 {
        usage::record(&path, &record(&format!("model/{index}")));
    }
    std::env::remove_var("LOCAL_ROUTER_USAGE_MAX_BYTES");

    // The log stayed small, and the older generation is kept beside it.
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size <= 400, "the log grew to {size} bytes");
    let old = path.with_extension("jsonl.1");
    assert!(old.exists(), "the old generation was not kept");
    assert!(!std::fs::read_to_string(&old).unwrap().is_empty());

    // The records that are still there are readable.
    for line in std::fs::read_to_string(&path).unwrap().lines() {
        let value: serde_json::Value = serde_json::from_str(line).expect("valid json");
        assert!(value["model"].as_str().unwrap().starts_with("model/"));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_money_sums_up_per_provider_and_per_day() {
    let mut a = record("workbuddy/hy3");
    a.provider = "workbuddy".into();
    a.output_tokens = 1000;
    a.cost_usd = Some(0.01);
    a.credits = Some(1.5);
    let mut b = record("openai/gpt-5.2");
    b.provider = "openai".into();
    b.output_tokens = 2000;
    b.cost_usd = Some(0.02);
    let failed = record("openai/broken");
    let failed = UsageRecord {
        ok: false,
        ..failed
    };

    let records = vec![a, b, failed];
    let totals = usage::totals(&records);
    assert!((totals.cost_usd - 0.03).abs() < 1e-9);
    assert!((totals.credits - 1.5).abs() < 1e-9);
    // A failed request cost nothing and is not reported as unpriced:
    // "unpriced" means "answered, but we cannot say what it cost".
    assert_eq!(totals.unpriced, 0);

    let by_provider = usage::group_by(&records, |r| r.provider.clone());
    let workbuddy = by_provider.iter().find(|(p, _)| p == "workbuddy").unwrap();
    assert!((workbuddy.1.credits - 1.5).abs() < 1e-9);
    let openai = by_provider.iter().find(|(p, _)| p == "openai").unwrap();
    assert!((openai.1.cost_usd - 0.02).abs() < 1e-9);

    // A record without any price is counted as unpriced, not silently zero.
    let mut unpriced = record("mystery/model");
    unpriced.priced = false;
    unpriced.output_tokens = 500;
    let records = vec![unpriced];
    let totals = usage::totals(&records);
    assert_eq!(totals.unpriced, 1);
    assert!(totals.cost_usd.abs() < 1e-9);
}

#[test]
fn the_builtin_table_prices_a_workbuddy_request_in_both_units() {
    let cost = local_router::pricing::cost_of("workbuddy", "hy3", 1_000_000, 2_000_000, 0, 0);
    let usd = cost.usd.expect("hy3 has a dollar price");
    assert!((usd - (0.28 + 2.0 * 1.12)).abs() < 1e-9);
    let credits = cost.credits.expect("hy3 has a credit rate");
    assert!((credits - 3.0 * 120.0).abs() < 1e-9);

    // An unknown model is unpriced, which the usage page reports.
    let cost = local_router::pricing::cost_of("workbuddy", "no-such-model", 1, 1, 0, 0);
    assert!(cost.usd.is_none());
}
