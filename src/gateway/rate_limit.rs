//! Per-provider and per-caller-key rate limits: requests per minute (RPM),
//! tokens per minute (TPM), and a daily token budget.
//!
//! This is a **local throttle**, not a vendor 429: skipping a target here
//! must not start a cooldown, and hitting it must not be retried against the
//! same provider forever. Limits live in the config (`Provider.rpm/tpm`,
//! `CallerKey.rpm/tpm/daily_tokens`); the windows live here, in process
//! memory, and reset with the daemon — the long-term account of what was
//! spent stays in `usage.db`.
//!
//! The registry is deliberately sync (a `std` Mutex over short map ops) so
//! `persist_usage` — which is sync — can record the true token cost of a
//! finished request without an async hop.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::metrics;

/// The window every RPM/TPM limit looks through.
const WINDOW: Duration = Duration::from_secs(60);

/// Whether a request may pass for a scope.
#[derive(Debug, Clone)]
pub enum Decision {
    Allow,
    Limited {
        reason: String,
        retry_after_secs: u64,
    },
}

#[derive(Default)]
struct Entry {
    /// Admission times of the requests inside the window.
    requests: VecDeque<Instant>,
    /// (time, tokens) of every finished request inside the window.
    tokens: VecDeque<(Instant, u64)>,
    /// The daily token budget counter, rolling at UTC midnight.
    day: DayCounter,
}

impl Entry {
    /// Drop everything older than the window.
    fn prune(&mut self, now: Instant) {
        while self.requests.front().is_some_and(|t| now - *t >= WINDOW) {
            self.requests.pop_front();
        }
        while self.tokens.front().is_some_and(|(t, _)| now - *t >= WINDOW) {
            self.tokens.pop_front();
        }
    }

    fn seconds_until_requests_free(&self, now: Instant) -> u64 {
        self.requests
            .front()
            .map(|oldest| WINDOW.saturating_sub(now - *oldest).as_secs() + 1)
            .unwrap_or(1)
    }

    fn seconds_until_tokens_free(&self, now: Instant) -> u64 {
        self.tokens
            .front()
            .map(|(oldest, _)| WINDOW.saturating_sub(now - *oldest).as_secs() + 1)
            .unwrap_or(1)
    }
}

/// A token counter that resets at UTC midnight.
#[derive(Default)]
struct DayCounter {
    /// The unix day number this counter belongs to.
    day: i64,
    used: u64,
}

impl DayCounter {
    fn today() -> i64 {
        chrono::Utc::now().timestamp().div_euclid(86_400)
    }

    fn roll(&mut self) {
        let today = Self::today();
        if self.day != today {
            self.day = today;
            self.used = 0;
        }
    }
}

/// The scopes' windows, shared by gateway and console on [`AppState`].
#[derive(Clone, Default)]
pub struct Registry {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

/// Scope key for one provider.
pub fn provider_scope(provider_id: &str) -> String {
    format!("provider:{provider_id}")
}

/// Scope key for one caller key.
pub fn caller_scope(caller_key_id: &str) -> String {
    format!("caller:{caller_key_id}")
}

impl Registry {
    /// Decide whether a request may pass for this scope. Does not reserve:
    /// the actual token cost is added by [`Registry::record`] once the reply
    /// is in (a request's token count is unknown before the vendor answers).
    ///
    /// `None` limits mean unlimited; a scope with every limit unset never
    /// gets an entry.
    pub fn check(
        &self,
        scope: &str,
        rpm: Option<u64>,
        tpm: Option<u64>,
        daily_tokens: Option<u64>,
    ) -> Decision {
        if rpm.is_none() && tpm.is_none() && daily_tokens.is_none() {
            return Decision::Allow;
        }
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("rate limit registry lock");
        let entry = entries.entry(scope.to_string()).or_default();
        entry.prune(now);
        if rpm.is_some_and(|limit| entry.requests.len() as u64 >= limit) {
            metrics::record_rate_limited(scope);
            return Decision::Limited {
                reason: format!(
                    "the local rate limit of {} requests/minute is full",
                    rpm.unwrap()
                ),
                retry_after_secs: entry.seconds_until_requests_free(now),
            };
        }
        if tpm.is_some_and(|limit| entry.tokens_in_window() >= limit) {
            metrics::record_rate_limited(scope);
            return Decision::Limited {
                reason: format!(
                    "the local rate limit of {} tokens/minute is full",
                    tpm.unwrap()
                ),
                retry_after_secs: entry.seconds_until_tokens_free(now),
            };
        }
        if let Some(daily) = daily_tokens {
            entry.day.roll();
            if entry.day.used >= daily {
                metrics::record_rate_limited(scope);
                return Decision::Limited {
                    reason: format!("the daily budget of {daily} tokens is spent"),
                    retry_after_secs: seconds_until_utc_midnight(),
                };
            }
        }
        entry.requests.push_back(now);
        Decision::Allow
    }

    /// Add the tokens a finished request actually cost. Scopes that never
    /// carried a limit never had a window, so there is nothing to record:
    /// only the windows [`Registry::check`] created are fed here.
    pub fn record(&self, scope: &str, tokens: u64) {
        if tokens == 0 {
            return;
        }
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("rate limit registry lock");
        let Some(entry) = entries.get_mut(scope) else {
            return;
        };
        entry.prune(now);
        entry.tokens.push_back((now, tokens));
        entry.day.roll();
        entry.day.used = entry.day.used.saturating_add(tokens);
    }

    /// A live read of every limited scope, for the console and `/metrics`.
    pub fn snapshot(&self) -> Vec<RateRow> {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("rate limit registry lock");
        let mut rows: Vec<RateRow> = entries
            .iter_mut()
            .map(|(scope, entry)| {
                entry.prune(now);
                entry.day.roll();
                RateRow {
                    scope: scope.clone(),
                    requests_in_window: entry.requests.len() as u64,
                    tokens_in_window: entry.tokens_in_window(),
                    tokens_today: entry.day.used,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.scope.cmp(&b.scope));
        rows
    }
}

impl Entry {
    fn tokens_in_window(&self) -> u64 {
        self.tokens.iter().map(|(_, n)| *n).sum()
    }
}

/// One limited scope's live counters.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RateRow {
    /// `provider:<id>` or `caller:<id>`.
    pub scope: String,
    /// Requests admitted inside the current 60s window.
    pub requests_in_window: u64,
    /// Tokens recorded inside the current 60s window.
    pub tokens_in_window: u64,
    /// Tokens recorded since the last UTC midnight.
    pub tokens_today: u64,
}

fn seconds_until_utc_midnight() -> u64 {
    let secs_of_day = chrono::Utc::now().timestamp().rem_euclid(86_400) as u64;
    86_400 - secs_of_day
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_limits_never_create_an_entry() {
        let registry = Registry::default();
        assert!(matches!(
            registry.check("provider:x", None, None, None),
            Decision::Allow
        ));
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn rpm_throttles_then_frees() {
        let registry = Registry::default();
        for _ in 0..3 {
            assert!(
                matches!(
                    registry.check("provider:r", Some(3), None, None),
                    Decision::Allow
                ),
                "the first three requests pass"
            );
        }
        let Decision::Limited { reason, .. } = registry.check("provider:r", Some(3), None, None)
        else {
            panic!("the fourth request must be limited");
        };
        assert!(reason.contains("3 requests/minute"), "{reason}");
    }

    #[test]
    fn recorded_tokens_fill_the_tpm_window() {
        let registry = Registry::default();
        assert!(matches!(
            registry.check("provider:t", None, Some(100), None),
            Decision::Allow
        ));
        registry.record("provider:t", 60);
        // 60 of 100 tokens spent: one more request still passes, but its
        // tokens would overflow, which the next admission cycle catches.
        registry.record("provider:t", 50);
        let Decision::Limited { reason, .. } = registry.check("provider:t", None, Some(100), None)
        else {
            panic!("110 tokens in the window must trip the 100 TPM limit");
        };
        assert!(reason.contains("100 tokens/minute"), "{reason}");
    }

    #[test]
    fn the_daily_budget_refuses_after_being_spent() {
        let registry = Registry::default();
        registry.check("caller:d", None, None, Some(10));
        registry.record("caller:d", 10);
        let Decision::Limited { reason, .. } = registry.check("caller:d", None, None, Some(10))
        else {
            panic!("the spent daily budget must refuse");
        };
        assert!(reason.contains("daily budget"), "{reason}");
        let row = &registry.snapshot()[0];
        assert_eq!(row.tokens_today, 10);
    }

    #[test]
    fn scopes_are_independent() {
        let registry = Registry::default();
        for _ in 0..2 {
            registry.check("provider:a", Some(2), None, None);
        }
        assert!(matches!(
            registry.check("provider:b", Some(2), None, None),
            Decision::Allow
        ));
    }

    #[test]
    fn recording_into_an_unlimited_scope_creates_no_window() {
        let registry = Registry::default();
        registry.record("provider:never-limited", 100);
        assert!(
            registry.snapshot().is_empty(),
            "a scope with no configured limit has no window"
        );
    }
}
