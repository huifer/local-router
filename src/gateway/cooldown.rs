//! A cooldown per upstream: the quiet time after a vendor says "429".

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const BASE_SECS: u64 = 60;
const MAX_SECS: u64 = 600;

#[derive(Debug, Clone, Copy)]
struct Entry {
    until: Instant,
    strikes: u32,
}

/// Cooldowns owned by one running gateway. Keeping this on AppState means
/// parallel test gateways do not poison one another, while the web router
/// and gateway in the real process still share one map.
#[derive(Clone, Default)]
pub struct Registry {
    map: Arc<Mutex<BTreeMap<String, Entry>>>,
}

impl Registry {
    pub async fn is_cooling(&self, key: &str) -> bool {
        let mut map = self.map.lock().await;
        forget_expired(&mut map);
        map.contains_key(key)
    }

    pub async fn provider_is_cooling(&self, provider_id: &str) -> bool {
        self.is_cooling(provider_id).await
    }

    pub async fn account_is_cooling(&self, provider_id: &str, account: &str) -> bool {
        self.provider_is_cooling(provider_id).await
            || self.is_cooling(&account_key(provider_id, account)).await
    }

    pub async fn penalize(&self, provider_id: &str, account: Option<&str>) {
        self.penalize_after(provider_id, account, None).await
    }

    /// Cool exactly one stored API key. Stored keys are separate accounts
    /// with their own quota: one exhausted key must not park every other
    /// key of the same provider. No provider-wide entry is written, so the
    /// rest of the key pool keeps serving.
    pub async fn penalize_key(
        &self,
        provider_id: &str,
        key_id: &str,
        retry_after: Option<Duration>,
    ) {
        let mut map = self.map.lock().await;
        forget_expired(&mut map);
        put(
            &mut map,
            account_key(provider_id, key_id),
            Instant::now(),
            retry_after,
        );
    }

    /// The same, honouring the vendor's own `Retry-After`. A vendor saying
    /// "wait 5s" no longer costs the fixed 60s of quota; one saying "wait
    /// 1h" no longer gets poked after 60s. Exponential backoff still wins
    /// when it is longer, so repeated strikes are never rewarded.
    pub async fn penalize_after(
        &self,
        provider_id: &str,
        account: Option<&str>,
        retry_after: Option<Duration>,
    ) {
        let mut map = self.map.lock().await;
        forget_expired(&mut map);
        let now = Instant::now();
        put(&mut map, provider_id.to_string(), now, retry_after);
        if let Some(account) = account {
            put(
                &mut map,
                account_key(provider_id, account),
                now,
                retry_after,
            );
        }
    }

    pub async fn clear(&self, provider_id: &str, account: Option<&str>) {
        let mut map = self.map.lock().await;
        map.remove(provider_id);
        if let Some(account) = account {
            map.remove(&account_key(provider_id, account));
        }
    }

    pub async fn clear_all(&self) {
        self.map.lock().await.clear();
    }

    pub async fn snapshot(&self) -> Vec<CooldownRow> {
        let mut map = self.map.lock().await;
        forget_expired(&mut map);
        map.iter()
            .map(|(key, entry)| CooldownRow {
                key: key.clone(),
                seconds_left: entry
                    .until
                    .saturating_duration_since(Instant::now())
                    .as_secs(),
                strikes: entry.strikes,
            })
            .collect()
    }
}

// Process-global compatibility helpers for short-lived CLI commands and
// unit tests. The daemon itself uses AppState::cooldowns above.
fn global() -> &'static Registry {
    static GLOBAL: OnceLock<Registry> = OnceLock::new();
    GLOBAL.get_or_init(Registry::default)
}

pub async fn is_cooling(key: &str) -> bool {
    global().is_cooling(key).await
}
pub async fn provider_is_cooling(key: &str) -> bool {
    global().provider_is_cooling(key).await
}
pub async fn account_is_cooling(provider: &str, account: &str) -> bool {
    global().account_is_cooling(provider, account).await
}
pub async fn penalize(provider: &str, account: Option<&str>) {
    global().penalize(provider, account).await
}
pub async fn penalize_after(provider: &str, account: Option<&str>, retry_after: Option<Duration>) {
    global()
        .penalize_after(provider, account, retry_after)
        .await
}
pub async fn penalize_key(provider: &str, key: &str, retry_after: Option<Duration>) {
    global().penalize_key(provider, key, retry_after).await
}
pub async fn clear(provider: &str, account: Option<&str>) {
    global().clear(provider, account).await
}
pub async fn clear_all() {
    global().clear_all().await
}
pub async fn snapshot() -> Vec<CooldownRow> {
    global().snapshot().await
}

fn base() -> Duration {
    let secs = std::env::var("LOCAL_ROUTER_COOLDOWN_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(BASE_SECS);
    Duration::from_secs(secs)
}

fn max() -> Duration {
    let secs = std::env::var("LOCAL_ROUTER_COOLDOWN_MAX_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(MAX_SECS);
    Duration::from_secs(secs)
}

fn put(
    map: &mut BTreeMap<String, Entry>,
    key: String,
    now: Instant,
    retry_after: Option<Duration>,
) {
    let strikes = map.get(&key).map(|e| e.strikes + 1).unwrap_or(1);
    // Our own backoff is the floor. A vendor's explicit Retry-After wins
    // when it asks for longer (a quota window that outlives our max), but
    // is capped at a day so a malformed header cannot park a provider.
    let backoff = (base().as_secs().saturating_mul(1 << (strikes - 1).min(4))).min(max().as_secs());
    let secs = match retry_after {
        Some(d) => d.as_secs().min(86_400).max(backoff),
        None => backoff,
    };
    map.insert(
        key,
        Entry {
            until: now + Duration::from_secs(secs),
            strikes,
        },
    );
}

fn forget_expired(map: &mut BTreeMap<String, Entry>) {
    let now = Instant::now();
    map.retain(|_, entry| entry.until > now);
}

fn account_key(provider_id: &str, account: &str) -> String {
    format!("{provider_id}/{account}")
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CooldownRow {
    pub key: String,
    pub seconds_left: u64,
    pub strikes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_penalty_cools_the_provider_down_and_clears_again() {
        let registry = Registry::default();
        registry.penalize("mock-provider", Some("default")).await;
        assert!(registry.provider_is_cooling("mock-provider").await);
        assert!(
            registry
                .account_is_cooling("mock-provider", "default")
                .await
        );
        assert!(!registry.provider_is_cooling("other-provider").await);
        registry.clear("mock-provider", Some("default")).await;
        assert!(!registry.provider_is_cooling("mock-provider").await);
        assert!(
            !registry
                .account_is_cooling("mock-provider", "default")
                .await
        );
    }

    #[tokio::test]
    async fn repeated_penalties_stretch_the_cooldown() {
        let registry = Registry::default();
        registry.penalize("mock-stretch", None).await;
        registry.penalize("mock-stretch", None).await;
        let rows = registry.snapshot().await;
        let row = rows.iter().find(|r| r.key == "mock-stretch").unwrap();
        assert_eq!(row.strikes, 2);
        assert!(row.seconds_left > base().as_secs());
    }

    #[tokio::test]
    async fn retry_after_longer_than_backoff_is_honoured() {
        let registry = Registry::default();
        registry
            .penalize_after("vendor", None, Some(Duration::from_secs(3600)))
            .await;
        let rows = registry.snapshot().await;
        let row = rows.iter().find(|r| r.key == "vendor").unwrap();
        // One hour, not the 600s ceiling of our own backoff.
        assert!(row.seconds_left >= 3599, "{}", row.seconds_left);
    }

    #[tokio::test]
    async fn a_short_retry_after_never_beats_the_backoff_floor() {
        let registry = Registry::default();
        registry
            .penalize_after("vendor-short", None, Some(Duration::from_secs(5)))
            .await;
        let rows = registry.snapshot().await;
        let row = rows.iter().find(|r| r.key == "vendor-short").unwrap();
        assert!(row.seconds_left >= base().as_secs() - 1);
    }

    #[tokio::test]
    async fn a_key_penalty_does_not_convict_the_whole_provider() {
        let registry = Registry::default();
        registry.penalize_key("pool", "key-a", None).await;
        assert!(registry.account_is_cooling("pool", "key-a").await);
        assert!(!registry.account_is_cooling("pool", "key-b").await);
        assert!(!registry.provider_is_cooling("pool").await);
        registry.clear("pool", Some("key-a")).await;
        assert!(!registry.account_is_cooling("pool", "key-a").await);
    }
}
