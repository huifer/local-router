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
//! local-router's own 0600 file.

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
    },
}

fn store() -> &'static Arc<Mutex<BTreeMap<String, Pending>>> {
    static STORE: OnceLock<Arc<Mutex<BTreeMap<String, Pending>>>> = OnceLock::new();
    STORE.get_or_init(|| Arc::new(Mutex::new(BTreeMap::new())))
}

/// Remember a login in flight.
pub fn remember(provider_id: &str, pending: Pending) {
    let mut flows = store().lock().unwrap();
    flows.insert(provider_id.to_string(), pending);
}

/// Forget a login (it finished, or it failed).
pub fn forget(provider_id: &str) {
    let mut flows = store().lock().unwrap();
    flows.remove(provider_id);
}

/// The login in flight for a provider, if any.
pub fn peek(provider_id: &str) -> Option<Pending> {
    let flows = store().lock().unwrap();
    flows.get(provider_id).cloned()
}

/// The login in flight for a provider. Same as [`peek`]; kept for call
/// sites that read better with this name.
pub fn take_or_peek(provider_id: &str) -> Option<Pending> {
    peek(provider_id)
}
