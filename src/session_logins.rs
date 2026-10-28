//! The browser sign-ins that are in flight.
//!
//! Two kinds of upstream sign in through a browser and are then polled
//! rather than redirected back to us:
//!
//! - WorkBuddy: its own login page, then a poll on the session state;
//! - QwenWork（千问办公）: a device flow, then a poll on the device token.
//!
//! The console starts one of these, opens the vendor's page, and polls until
//! the vendor says the sign-in landed. The pending state lives here, in
//! memory: it dies with the process, and the finished session is written to
//! local-router's own file next to the config.
//!
//! Several sign-ins can be in flight for the same provider at once — each
//! one claims its own slot in the account pool, so a second sign-in never
//! overwrites the first account. The status endpoint therefore polls every
//! sign-in in flight and reports the first one that lands.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A login in flight.
#[derive(Clone)]
pub enum Pending {
    /// A WorkBuddy sign-in, polled by its session state.
    WorkBuddy { state: String, account: String },
    /// A QwenWork device flow, polled by its device state.
    QwenWork {
        state: crate::qwen::login::LoginState,
        /// The pool slot this sign-in fills.
        account: String,
    },
}

impl Pending {
    /// The account-pool slot this sign-in fills.
    pub fn slot(&self) -> &str {
        match self {
            Pending::WorkBuddy { account, .. } => account,
            Pending::QwenWork { account, .. } => account,
        }
    }
}

/// The sign-ins in flight, by provider.
type Flows = BTreeMap<String, Vec<Pending>>;

fn store() -> &'static Arc<Mutex<Flows>> {
    static STORE: OnceLock<Arc<Mutex<Flows>>> = OnceLock::new();
    STORE.get_or_init(|| Arc::new(Mutex::new(BTreeMap::new())))
}

/// Remember a login in flight.
pub fn remember(provider_id: &str, pending: Pending) {
    let mut flows = store().lock().unwrap();
    flows
        .entry(provider_id.to_string())
        .or_default()
        .push(pending);
}

/// Forget a login (it finished, or it failed).
pub fn forget(provider_id: &str) {
    let mut flows = store().lock().unwrap();
    flows.remove(provider_id);
}

/// Forget the sign-in that filled a given slot.
pub fn forget_slot(provider_id: &str, slot: &str) {
    let mut flows = store().lock().unwrap();
    if let Some(list) = flows.get_mut(provider_id) {
        list.retain(|pending| pending.slot() != slot);
        if list.is_empty() {
            flows.remove(provider_id);
        }
    }
}

/// The logins in flight for a provider.
pub fn peek_all(provider_id: &str) -> Vec<Pending> {
    let flows = store().lock().unwrap();
    flows.get(provider_id).cloned().unwrap_or_default()
}

/// The login in flight for a provider, if any.
pub fn peek(provider_id: &str) -> Option<Pending> {
    peek_all(provider_id).pop()
}

/// The login in flight for a provider. Same as [`peek`]; kept for call
/// sites that read better with this name.
pub fn take_or_peek(provider_id: &str) -> Option<Pending> {
    peek(provider_id)
}
