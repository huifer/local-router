//! Per-provider-key in-flight request limits.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Default)]
pub struct Registry {
    gates: Arc<Mutex<HashMap<String, Gate>>>,
}

struct Gate {
    limit: usize,
    semaphore: Arc<Semaphore>,
}

impl Registry {
    /// Wait for one permit. `None` or zero means unlimited.
    pub async fn acquire(&self, key: &str, limit: Option<usize>) -> Option<OwnedSemaphorePermit> {
        let limit = limit.filter(|n| *n > 0)?;
        let semaphore = {
            let mut gates = self.gates.lock().await;
            let gate = gates.entry(key.to_string()).or_insert_with(|| Gate {
                limit,
                semaphore: Arc::new(Semaphore::new(limit)),
            });
            if gate.limit != limit {
                *gate = Gate {
                    limit,
                    semaphore: Arc::new(Semaphore::new(limit)),
                };
            }
            gate.semaphore.clone()
        };
        Some(
            semaphore
                .acquire_owned()
                .await
                .expect("concurrency semaphore is never closed"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_gate_allows_only_the_configured_number() {
        let registry = Registry::default();
        let first = registry.acquire("p/key", Some(1)).await.unwrap();
        let pending = tokio::spawn({
            let registry = registry.clone();
            async move { registry.acquire("p/key", Some(1)).await }
        });
        assert!(!pending.is_finished());
        drop(first);
        assert!(pending.await.unwrap().is_some());
    }
}
