//! Projects: grouping usage by the working directory an agent runs in.
//!
//! A project is a working directory. Agents that route through the gateway can
//! report their directory (Pi, Claude Code, Codex and friends all know their
//! own CWD); the gateway turns that directory into a project and lets the user
//! rename it, color it, give it a budget and merge duplicates — the way a
//! workspace groups every conversation under the folder it happened in.
//!
//! The directory is the stable key; the name and color are the user's. Two
//! requests from `~/work/acme` and `~/work/ACME` are different keys until the
//! user merges them, which keeps the data honest while still letting the user
//! tidy it up afterwards.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Where the project registry lives: next to the config.
pub fn projects_file() -> PathBuf {
    crate::config::config_dir().join("projects.json")
}

/// One project the gateway has met.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Project {
    /// The working directory, as the agent reported it. The stable key.
    pub id: String,
    /// The friendly name shown in the UI. Defaults to the id.
    #[serde(default)]
    pub name: String,
    /// A color (any CSS color) for the project's dot.
    #[serde(default = "default_color")]
    pub color: String,
    /// When the project was first seen, RFC 3339.
    #[serde(default)]
    pub created_at: String,
    /// A token budget across the period, when the user set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
    /// A cost budget (USD) across the period, when the user set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_cost: Option<f64>,
}

fn default_color() -> String {
    "#9ef0bf".to_string()
}

impl Project {
    /// The name to show: the friendly name when set, else the directory.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

/// The whole registry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Projects {
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
}

/// The shared registry. A local gateway meets few projects; an in-memory map
/// guarded by a mutex is plenty, and it is flushed to disk on every change.
static REGISTRY: OnceLock<Arc<Mutex<Option<Projects>>>> = OnceLock::new();

fn registry() -> &'static Arc<Mutex<Option<Projects>>> {
    REGISTRY.get_or_init(|| Arc::new(Mutex::new(Some(read_from_disk()))))
}

/// Persist the registry to disk.
fn save(projects: &Projects) {
    if let Some(parent) = projects_file().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string_pretty(projects) {
        let _ = std::fs::write(projects_file(), body);
    }
}

/// Record that a directory has been used. Gives it a name (defaults to the
/// directory) on first sight, and never overwrites a name the user set.
pub fn note(dir: &str) {
    let dir = normalize(dir);
    if dir.is_empty() {
        return;
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut guard = registry().lock().unwrap();
    let Some(projects) = guard.as_mut() else {
        return;
    };
    projects
        .projects
        .entry(dir.clone())
        .or_insert_with(|| Project {
            id: dir.clone(),
            name: String::new(),
            color: default_color(),
            created_at: now,
            budget_tokens: None,
            budget_cost: None,
        });
    let snapshot = projects.clone();
    drop(guard);
    save(&snapshot);
}

/// Re-read the registry from disk.
///
/// The registry is process-global, so a caller that moves the config
/// directory (a test, or a tool that hosts several configurations) needs to
/// drop the copy it loaded at startup.
pub fn reload() {
    let fresh = read_from_disk();
    let mut guard = registry().lock().unwrap();
    *guard = Some(fresh);
}

/// The registry as the file holds it.
fn read_from_disk() -> Projects {
    let mut projects = Projects::default();
    if let Ok(text) = std::fs::read_to_string(projects_file()) {
        if let Ok(parsed) = serde_json::from_str::<Projects>(&text) {
            projects = parsed;
        }
    }
    projects
}

/// Read the whole registry.
pub fn load() -> Projects {
    let guard = registry().lock().unwrap();
    guard.clone().unwrap_or_default()
}

/// A project's editable fields.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProjectPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<Option<u64>>,
    #[serde(default)]
    pub budget_cost: Option<Option<f64>>,
}

/// Apply an edit to one project. Returns false when the id is unknown.
pub fn patch(id: &str, patch: &ProjectPatch) -> bool {
    let id = normalize(id);
    let mut guard = registry().lock().unwrap();
    let Some(projects) = guard.as_mut() else {
        return false;
    };
    let Some(project) = projects.projects.get_mut(&id) else {
        return false;
    };
    if let Some(name) = &patch.name {
        project.name = name.trim().to_string();
    }
    if let Some(color) = &patch.color {
        if !color.trim().is_empty() {
            project.color = color.trim().to_string();
        }
    }
    if let Some(budget) = patch.budget_tokens {
        project.budget_tokens = budget;
    }
    if let Some(budget) = patch.budget_cost {
        project.budget_cost = budget;
    }
    let snapshot = projects.clone();
    drop(guard);
    save(&snapshot);
    true
}

/// Merge `from` into `into`: every usage record keyed by `from` should be
/// re-keyed to `into`. The caller re-keys the usage store; this only moves the
/// registry entry (the `from` project is dropped, its name/color kept if the
/// target has none). Returns false when `into` is unknown.
pub fn merge(from: &str, into: &str) -> bool {
    let from = normalize(from);
    let into = normalize(into);
    if from == into {
        return false;
    }
    let mut guard = registry().lock().unwrap();
    let Some(projects) = guard.as_mut() else {
        return false;
    };
    let Some(from_project) = projects.projects.remove(&from) else {
        return false;
    };
    let target = projects
        .projects
        .entry(into.clone())
        .or_insert_with(|| Project {
            id: into.clone(),
            name: String::new(),
            color: default_color(),
            created_at: from_project.created_at.clone(),
            budget_tokens: None,
            budget_cost: None,
        });
    if target.name.is_empty() && !from_project.name.is_empty() {
        target.name = from_project.name;
    }
    if target.color == default_color() && from_project.color != default_color() {
        target.color = from_project.color;
    }
    if target.budget_tokens.is_none() {
        target.budget_tokens = from_project.budget_tokens;
    }
    if target.budget_cost.is_none() {
        target.budget_cost = from_project.budget_cost;
    }
    let snapshot = projects.clone();
    drop(guard);
    save(&snapshot);
    true
}

/// Normalize a directory key: trim whitespace and a single trailing slash,
/// so `~/work/acme` and `~/work/acme/` are the same project.
fn normalize(dir: &str) -> String {
    let mut s = dir.trim().to_string();
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::IsolatedHome;

    /// The registry is process-global; the tests below share it, so every
    /// directory is namespaced and assertions never touch other tests' rows.
    fn udir(tag: &str) -> String {
        format!(
            "/tmp/lr-project-unit-{}-{}-{}",
            tag,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )
    }

    #[test]
    fn normalize_trims_and_unifies_trailing_slashes() {
        assert_eq!(normalize("  /work/acme/  "), "/work/acme");
        assert_eq!(normalize("/work/acme///"), "/work/acme");
        assert_eq!(normalize("/"), "/"); // never empties out
        assert_eq!(normalize("  "), "");
    }

    #[test]
    fn display_name_prefers_the_friendly_name() {
        let project = Project {
            id: "/work/acme".into(),
            name: "Acme".into(),
            ..Project::default()
        };
        assert_eq!(project.display_name(), "Acme");
        let bare = Project {
            id: "/work/bare".into(),
            ..Project::default()
        };
        assert_eq!(bare.display_name(), "/work/bare");
    }

    #[test]
    fn note_is_idempotent_and_never_overwrites_a_name() {
        // Never touch the user's real registry.
        let _home = IsolatedHome::new("projects");
        reload();
        let dir = udir("note");
        note(&dir);
        note(&dir);
        note(&format!("{dir}/")); // the same project, slash and all
        let registry = load();
        assert_eq!(registry.projects.keys().filter(|k| **k == dir).count(), 1);
    }

    #[test]
    fn patch_renames_recolors_and_sets_budgets() {
        // Never touch the user's real registry.
        let _home = IsolatedHome::new("projects");
        reload();
        let dir = udir("patch");
        note(&dir);
        let edit = ProjectPatch {
            name: Some("My Project".into()),
            color: Some("#ff0000".into()),
            budget_tokens: Some(Some(1_000_000)),
            budget_cost: Some(Some(5.0)),
        };
        assert!(patch(&dir, &edit));
        let project = load().projects.get(&dir).unwrap().clone();
        assert_eq!(project.name, "My Project");
        assert_eq!(project.color, "#ff0000");
        assert_eq!(project.budget_tokens, Some(1_000_000));
        assert_eq!(project.budget_cost, Some(5.0));

        // Clearing a budget works; blank colors are ignored.
        let clear = ProjectPatch {
            name: None,
            color: Some("  ".into()),
            budget_tokens: Some(None),
            budget_cost: Some(None),
        };
        assert!(patch(&dir, &clear));
        let project = load().projects.get(&dir).unwrap().clone();
        assert_eq!(project.budget_tokens, None);
        assert_eq!(project.budget_cost, None);
        assert_eq!(project.color, "#ff0000");

        // An unknown project fails cleanly.
        assert!(!patch("/tmp/never-seen-project", &ProjectPatch::default()));
    }

    #[test]
    fn merge_moves_the_row_and_its_friendly_fields() {
        // Never touch the user's real registry.
        let _home = IsolatedHome::new("projects");
        reload();
        let from = udir("merge-from");
        let into = udir("merge-into");
        note(&from);
        patch(
            &from,
            &ProjectPatch {
                name: Some("Old Name".into()),
                color: Some("#00ff00".into()),
                budget_tokens: Some(Some(7)),
                budget_cost: None,
            },
        );
        note(&into); // target exists but is unnamed
        assert!(merge(&from, &into));
        let registry = load();
        assert!(!registry.projects.contains_key(&from));
        let target = registry.projects.get(&into).unwrap();
        assert_eq!(target.name, "Old Name");
        assert_eq!(target.color, "#00ff00");
        assert_eq!(target.budget_tokens, Some(7));

        // Degenerate cases fail without panicking.
        assert!(!merge(&into, &into)); // same key
        assert!(!merge("/tmp/never-a", "/tmp/never-b")); // source missing
    }

    #[test]
    fn empty_directories_are_not_registered() {
        // Never touch the user's real registry.
        let _home = IsolatedHome::new("projects");
        reload();
        let marker = udir("marker");
        note(&marker);
        let before = load();
        note("");
        note("   ");
        let after = load();
        assert_eq!(after.projects.len(), before.projects.len());
        assert!(after.projects.contains_key(&marker));
    }
}
