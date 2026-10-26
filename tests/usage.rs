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
