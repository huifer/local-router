//! Short status alerts, sent to a webhook the user configured.
//!
//! The gateway deliberately does not export request logs or prompts. These
//! events are state changes only — a budget being spent, a provider going
//! into cooldown — and carry no caller text, keys or model input. Delivery
//! is best effort: an alert is fire-and-forget with a short timeout and
//! repeated events are de-duplicated so a retry storm cannot become an
//! alert storm.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The body POSTed to the webhook. Works with ntfy (plain `message`),
/// Slack incoming webhooks (`text`) and 飞书 custom bots (`msg_type`/content).
fn payload(kind: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "topic": "local-router",
        "title": format!("local-router: {kind}"),
        "message": message,
        "text": message,
        "msg_type": "text",
        "content": {"text": message},
        "kind": kind,
        "at": chrono::Utc::now().to_rfc3339(),
    })
}

/// Remember which alerts already fired, so one hot loop does not send the
/// same notification every request. Entries expire after the quiet window.
#[derive(Default)]
struct Seen {
    keys: HashSet<String>,
    at: std::collections::HashMap<String, Instant>,
}

/// A webhook sender. Cloneable and cheap to share with the gateway.
#[derive(Clone)]
pub struct Alerts {
    seen: Arc<Mutex<Seen>>,
    client: reqwest::Client,
}

impl Default for Alerts {
    fn default() -> Self {
        Alerts {
            seen: Arc::new(Mutex::new(Seen::default())),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

impl Alerts {
    /// Emit an alert unless the same `dedup_key` fired inside `quiet`.
    /// `endpoint` is read from the live config, so turning alerts off in
    /// the console takes effect on the very next event.
    pub fn emit(&self, endpoint: Option<&str>, kind: &str, message: &str, dedup_key: &str) {
        let Some(url) = endpoint
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
        else {
            return;
        };
        if !self.should_emit_quiet(
            dedup_key,
            if kind.starts_with("budget") {
                Duration::from_secs(24 * 3_600)
            } else {
                Duration::from_secs(15 * 60)
            },
        ) {
            return;
        }
        let body = payload(kind, message);
        let client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.post(&url).json(&body).send().await;
        });
    }

    /// De-duplication, separated so it can be tested without a network.
    fn should_emit_quiet(&self, dedup_key: &str, quiet: Duration) -> bool {
        let mut seen = self.seen.lock().expect("alert dedup lock");
        let now = Instant::now();
        seen.at.retain(|_, at| now.duration_since(*at) < quiet);
        seen.keys = seen.at.keys().cloned().collect();
        if seen.at.contains_key(dedup_key) {
            return false;
        }
        seen.at.insert(dedup_key.to_string(), now);
        seen.keys.insert(dedup_key.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duplicate_event_is_suppressed() {
        let alerts = Alerts::default();
        assert!(alerts.should_emit_quiet("caller:k:80", Duration::from_secs(900)));
        assert!(!alerts.should_emit_quiet("caller:k:80", Duration::from_secs(900)));
        assert!(alerts.should_emit_quiet("cooldown:p", Duration::from_secs(900)));
        assert!(alerts.seen.lock().unwrap().at.contains_key("caller:k:80"));
        // With no endpoint, emit is a pure no-op (no task, no network).
        alerts.emit(None, "budget", "80% spent", "nothing");
        assert!(!alerts.seen.lock().unwrap().at.contains_key("nothing"));
    }

    #[test]
    fn payload_has_a_text_field_for_every_common_webhook() {
        let v = payload("cooldown", "vendor is cooling down");
        assert_eq!(v["text"], "vendor is cooling down");
        assert_eq!(v["content"]["text"], "vendor is cooling down");
        assert_eq!(v["kind"], "cooldown");
    }
}
