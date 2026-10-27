//! Test helpers: keep the tests away from the user's real data.
//!
//! Several modules (projects, sessions, the config store) resolve their
//! files through `config_dir()`, which honours `LOCAL_ROUTER_HOME`. A test
//! that forgets to point that somewhere else writes into the user's own
//! `~/.config/local-router` — which is exactly what happened once. Every
//! test that touches the filesystem goes through [`IsolatedHome`], which
//! redirects the config dir to a temp directory for the test's lifetime and
//! puts the old value back afterwards.
//!
//! The environment is process-global, so the tests that change it take a
//! lock ([`lock`]) and run one at a time.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// One lock for every test that changes the environment.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Take the environment lock for the duration of a test.
pub fn lock() -> MutexGuard<'static, ()> {
    env_lock().lock().unwrap_or_else(|e| e.into_inner())
}

/// A config directory of its own, for one test.
///
/// Holds the environment lock while alive; on drop it restores the previous
/// `LOCAL_ROUTER_HOME` and removes the directory.
pub struct IsolatedHome {
    _guard: MutexGuard<'static, ()>,
    dir: PathBuf,
    previous: Option<String>,
}

impl IsolatedHome {
    /// A fresh config directory for this test.
    pub fn new(tag: &str) -> Self {
        let guard = lock();
        let dir =
            std::env::temp_dir().join(format!("local-router-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the test's config dir");
        let previous = std::env::var("LOCAL_ROUTER_HOME").ok();
        std::env::set_var("LOCAL_ROUTER_HOME", &dir);
        IsolatedHome {
            _guard: guard,
            dir,
            previous,
        }
    }

    /// The directory this test may write to.
    pub fn path(&self) -> &std::path::Path {
        &self.dir
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("LOCAL_ROUTER_HOME", value),
            None => std::env::remove_var("LOCAL_ROUTER_HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
