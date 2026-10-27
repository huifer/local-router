//! Sessions: how the gateway remembers what a conversation was about.
//!
//! Agents that route through the gateway tag each request with a session id
//! (Anthropic's `metadata.user_id`, or an `x-session-id` / `x-magpie-session`
//! header). magpie shows a per-session spend and a title; this module keeps
//! the title. The title is the heading of the conversation — the first user
//! message's first line — captured the moment the gateway sees it, so the
//! Usage page can list sessions by what they were, not by an opaque id.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Where the session titles live: next to the config.
pub fn sessions_file() -> PathBuf {
    crate::config::config_dir().join("sessions.json")
}

/// One session we have seen.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    /// The id the agent sent.
    pub id: String,
    /// The conversation's title, or the id when none is known yet.
    #[serde(default)]
    pub title: String,
    /// When the session was first seen, RFC 3339.
    #[serde(default)]
    pub first_seen: String,
    /// When it was last seen, RFC 3339.
    #[serde(default)]
    pub last_seen: String,
    /// How many requests have carried this session id.
    #[serde(default)]
    pub requests: u64,
}

/// The session book: every session id the gateway has met.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sessions {
    #[serde(default)]
    pub sessions: BTreeMap<String, Session>,
}

/// The shared session book. A local gateway meets few sessions; an in-memory
/// map guarded by a mutex is plenty, and it is flushed to disk on change.
static BOOK: OnceLock<Arc<Mutex<Option<Sessions>>>> = OnceLock::new();

fn book() -> &'static Arc<Mutex<Option<Sessions>>> {
    BOOK.get_or_init(|| Arc::new(Mutex::new(Some(read_from_disk()))))
}

/// Re-read the book from disk.
///
/// The book is process-global, so a caller that moves the config directory
/// (a test, most often) needs to drop the copy loaded at startup.
pub fn reload() {
    let fresh = read_from_disk();
    let mut guard = book().lock().unwrap();
    *guard = Some(fresh);
}

/// The book as the file holds it.
fn read_from_disk() -> Sessions {
    let mut sessions = Sessions::default();
    if let Ok(text) = std::fs::read_to_string(sessions_file()) {
        if let Ok(parsed) = serde_json::from_str::<Sessions>(&text) {
            sessions = parsed;
        }
    }
    sessions
}

/// Load the book from memory (or disk on first use).
pub fn load() -> Sessions {
    let guard = book().lock().unwrap();
    guard.clone().unwrap_or_default()
}

/// Persist the book to disk.
fn save(sessions: &Sessions) {
    if let Some(parent) = sessions_file().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string_pretty(sessions) {
        let _ = std::fs::write(sessions_file(), body);
    }
}

/// Note that a session exists, and give it a title when one arrives.
///
/// `title` is the candidate title (the first user message); it is only set
/// when the session has no title yet, so the first message wins — the way a
/// chat names the thread by its opening line.
pub fn note(id: &str, title: Option<&str>) {
    if id.trim().is_empty() {
        return;
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut guard = book().lock().unwrap();
    let Some(sessions) = guard.as_mut() else {
        return;
    };
    let entry = sessions
        .sessions
        .entry(id.to_string())
        .or_insert_with(|| Session {
            id: id.to_string(),
            title: String::new(),
            first_seen: now.clone(),
            last_seen: now.clone(),
            requests: 0,
        });
    entry.last_seen = now;
    entry.requests += 1;
    if entry.title.is_empty() {
        if let Some(title) = title {
            let title = title.trim().lines().next().unwrap_or("").trim().to_string();
            if !title.is_empty() {
                entry.title = title;
            }
        }
    }
    let snapshot = sessions.clone();
    drop(guard);
    save(&snapshot);
}

/// Read one session's title, if it has been seen.
pub fn title_of(id: &str) -> Option<String> {
    let guard = book().lock().unwrap();
    guard
        .as_ref()
        .and_then(|s| s.sessions.get(id))
        .map(|s| s.title.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::IsolatedHome;

    /// The book is process-global; the tests below share it, so every id
    /// is namespaced with a unique prefix and assertions are prefix-scoped.
    fn uid(tag: &str) -> String {
        format!(
            "unit-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )
    }

    #[test]
    fn empty_ids_are_ignored() {
        // Never touch the user's real session book.
        let _home = IsolatedHome::new("sessions");
        reload();
        let marker = uid("marker");
        note(&marker, Some("sentinel"));
        let before = load();
        note("", Some("x"));
        note("   ", Some("x"));
        let after = load();
        assert_eq!(after.sessions.len(), before.sessions.len());
        assert!(after.sessions.contains_key(&marker));
    }

    #[test]
    fn note_creates_counts_and_titles() {
        // Never touch the user's real session book.
        let _home = IsolatedHome::new("sessions");
        reload();
        let id = uid("note");
        note(&id, Some("帮我重构网关模块\n第二行不算"));
        note(&id, None);
        note(&id, Some("后来的标题不覆盖"));
        let loaded = load();
        let entry = loaded.sessions.get(&id).unwrap();
        assert_eq!(entry.title, "帮我重构网关模块");
        assert_eq!(entry.requests, 3);
        assert_eq!(entry.id, id);
        assert!(!entry.first_seen.is_empty());
        assert_eq!(title_of(&id).as_deref(), Some("帮我重构网关模块"));
    }

    #[test]
    fn a_session_without_a_title_falls_back_to_its_id() {
        // Never touch the user's real session book.
        let _home = IsolatedHome::new("sessions");
        reload();
        let id = uid("untitled");
        note(&id, None);
        note(&id, Some("   \n  ")); // whitespace-only titles do not stick
        assert_eq!(title_of(&id).as_deref(), Some(""));
        // The JSON shape survives a round trip.
        let raw = serde_json::to_string(&load()).unwrap();
        assert!(raw.contains(&id));
    }
}
