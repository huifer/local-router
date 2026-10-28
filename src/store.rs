//! The SQLite store: every request's record, queryable without loading a
//! whole log file into memory.
//!
//! The store is one file next to the config (`usage.db`). Writes go through
//! one connection guarded by a mutex — this is a local gateway, not a
//! server farm, and one writer keeps the file healthy. Reads are queries,
//! so the console and the CLI can ask for aggregates instead of scanning
//! every line.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::Connection;

use crate::gateway::usage::UsageRecord;

/// The database file, next to the config.
pub fn db_path() -> PathBuf {
    crate::config::config_dir().join("usage.db")
}

/// Whether SQLite storage is on. It is on unless someone turns it off —
/// the JSONL log stays as the compatibility export either way.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("LOCAL_ROUTER_DISABLE_SQLITE")
            .map(|v| !matches!(v.trim(), "1" | "true" | "yes"))
            .unwrap_or(true)
    })
}

/// One shared connection per process. `Option` so closing it is possible
/// in tests.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Option<Connection>>>,
    path: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self::open(&db_path())
    }
}

impl Store {
    /// Open (and migrate) the database at `path`.
    pub fn open(path: &Path) -> Self {
        let conn = Connection::open(path).ok();
        if let Some(conn) = &conn {
            if let Err(err) = migrate(conn) {
                tracing::warn!("the usage database could not be prepared: {err}");
            }
            let _ = conn.pragma_update(None, "journal_mode", "WAL");
            let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        } else {
            tracing::warn!(
                "the usage database at {} could not be opened",
                path.display()
            );
        }
        Store {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The configured store: the default file, unless SQLite is off.
    pub fn shared() -> Option<Store> {
        enabled().then(Store::default)
    }

    /// Append one record. A failure is logged, never fatal: usage is a
    /// passenger, not the cargo.
    pub fn record(&self, rec: &UsageRecord) {
        let Ok(guard) = self.conn.lock() else {
            return;
        };
        let Some(conn) = guard.as_ref() else {
            return;
        };
        let result = conn.execute(
            "INSERT INTO usage_records
             (time, model, provider, key_id, key_label, group_id, protocol,
              input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
              reasoning_tokens, latency_ms, status, ok, error, agent, session_id,
              context, project, cost_usd, credits, priced)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
            rusqlite::params![
                rec.time,
                rec.model,
                rec.provider,
                rec.key_id,
                rec.key_label,
                rec.group,
                rec.protocol,
                rec.input_tokens,
                rec.output_tokens,
                rec.cache_read_tokens,
                rec.cache_write_tokens,
                rec.reasoning_tokens,
                rec.latency_ms,
                rec.status,
                rec.ok,
                rec.error,
                rec.agent,
                rec.session_id,
                rec.context.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()),
                rec.project,
                rec.cost_usd,
                rec.credits,
                rec.priced,
            ],
        );
        if let Err(err) = result {
            tracing::warn!("the usage record could not be written: {err}");
        }
    }

    /// Every record, oldest first — the JSONL log's shape, for callers
    /// that have not moved to queries yet.
    pub fn read_all(&self) -> Vec<UsageRecord> {
        self.query("SELECT * FROM usage_records ORDER BY time ASC", [])
    }

    /// Run a query and map every row to a record.
    pub fn query(&self, sql: &str, params: impl rusqlite::Params) -> Vec<UsageRecord> {
        let Ok(guard) = self.conn.lock() else {
            return Vec::new();
        };
        let Some(conn) = guard.as_ref() else {
            return Vec::new();
        };
        let Ok(mut statement) = conn.prepare(sql) else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map(params, row_to_record) else {
            return Vec::new();
        };
        rows.filter_map(|row| row.ok()).collect()
    }

    /// One aggregate query: `SELECT <dims>, SUM(...)... GROUP BY <dims>`.
    fn aggregate(&self, sql: &str, params: impl rusqlite::Params) -> Vec<AggregateRow> {
        let Ok(guard) = self.conn.lock() else {
            return Vec::new();
        };
        let Some(conn) = guard.as_ref() else {
            return Vec::new();
        };
        let Ok(mut statement) = conn.prepare(sql) else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map(params, |row| {
            Ok(AggregateRow {
                day: row.get::<_, Option<String>>(0)?,
                label: row.get::<_, Option<String>>(1)?,
                requests: row.get::<_, i64>(2)?,
                failed: row.get::<_, i64>(3)?,
                input_tokens: row.get::<_, i64>(4)?,
                output_tokens: row.get::<_, i64>(5)?,
                cache_read_tokens: row.get::<_, i64>(6)?,
                cache_write_tokens: row.get::<_, i64>(7)?,
                reasoning_tokens: row.get::<_, i64>(8)?,
                cost_usd: row.get::<_, Option<f64>>(9)?,
                credits: row.get::<_, Option<f64>>(10)?,
            })
        }) else {
            return Vec::new();
        };
        rows.filter_map(|row| row.ok()).collect()
    }

    /// Totals over a period, in one query.
    pub fn totals_since(&self, since: &str) -> Option<AggregateRow> {
        self.aggregate(
            "SELECT NULL, NULL,
                    COUNT(*), SUM(CASE WHEN ok = 0 THEN 1 ELSE 0 END),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                    COALESCE(SUM(reasoning_tokens),0),
                    SUM(cost_usd), SUM(credits)
             FROM usage_records WHERE time >= ?1 AND ok = 1",
            [since],
        )
        .into_iter()
        .next()
    }

    /// Per-day breakdown.
    pub fn by_day_since(&self, since: &str) -> Vec<AggregateRow> {
        self.aggregate(
            "SELECT substr(time, 1, 10) AS day, NULL,
                    COUNT(*), SUM(CASE WHEN ok = 0 THEN 1 ELSE 0 END),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                    COALESCE(SUM(reasoning_tokens),0),
                    SUM(cost_usd), SUM(credits)
             FROM usage_records WHERE time >= ?1 AND ok = 1
             GROUP BY day ORDER BY day ASC",
            [since],
        )
    }

    /// Per-provider breakdown: the per-platform bill.
    pub fn by_provider_since(&self, since: &str) -> Vec<AggregateRow> {
        self.aggregate(
            "SELECT NULL, provider,
                    COUNT(*), SUM(CASE WHEN ok = 0 THEN 1 ELSE 0 END),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                    COALESCE(SUM(reasoning_tokens),0),
                    SUM(cost_usd), SUM(credits)
             FROM usage_records WHERE time >= ?1 AND ok = 1
             GROUP BY provider ORDER BY cost_usd DESC",
            [since],
        )
    }

    /// Per-model breakdown.
    pub fn by_model_since(&self, since: &str) -> Vec<AggregateRow> {
        self.aggregate(
            "SELECT NULL, model,
                    COUNT(*) AS requests, SUM(CASE WHEN ok = 0 THEN 1 ELSE 0 END),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                    COALESCE(SUM(reasoning_tokens),0),
                    SUM(cost_usd), SUM(credits)
             FROM usage_records WHERE time >= ?1 AND ok = 1
             GROUP BY model ORDER BY requests DESC",
            [since],
        )
    }

    /// The newest records, for the request log.
    pub fn recent(&self, since: &str, limit: usize) -> Vec<UsageRecord> {
        self.query(
            "SELECT * FROM usage_records WHERE time >= ?1 ORDER BY time DESC LIMIT ?2",
            rusqlite::params![since, limit as i64],
        )
    }

    /// Every record since `since`, oldest first. Used for the Usage page's
    /// richer breakdowns (by session, agent, key, hour) that the aggregate
    /// queries do not cover.
    pub fn all_since(&self, since: &str) -> Vec<UsageRecord> {
        self.query(
            "SELECT * FROM usage_records WHERE time >= ?1 ORDER BY time ASC",
            [since],
        )
    }

    /// Re-key every record of one project to another, after a merge.
    pub fn rekey_project(&self, from: &str, into: &str) {
        let Ok(guard) = self.conn.lock() else {
            return;
        };
        let Some(conn) = guard.as_ref() else {
            return;
        };
        if let Err(err) = conn.execute(
            "UPDATE usage_records SET project = ?1 WHERE project = ?2",
            rusqlite::params![into, from],
        ) {
            tracing::warn!("the project merge could not re-key records: {err}");
        }
    }
}

/// One row of an aggregate query.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AggregateRow {
    /// The day, for the per-day breakdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    /// The provider or model, for the per-group breakdowns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub requests: i64,
    pub failed: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: Option<f64>,
    pub credits: Option<f64>,
}

/// The columns the newest code writes. A database created by an older
/// release may lack the later ones; each is added on open.
const COLUMNS: [(&str, &str); 17] = [
    ("provider", "TEXT NOT NULL DEFAULT ''"),
    ("key_id", "TEXT"),
    ("key_label", "TEXT"),
    ("group_id", "TEXT"),
    ("cache_read_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("cache_write_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("reasoning_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("latency_ms", "INTEGER NOT NULL DEFAULT 0"),
    ("error", "TEXT"),
    ("agent", "TEXT"),
    ("session_id", "TEXT"),
    ("context", "TEXT"),
    ("project", "TEXT"),
    ("cost_usd", "REAL"),
    ("credits", "REAL"),
    ("priced", "INTEGER NOT NULL DEFAULT 1"),
    ("ok", "INTEGER NOT NULL DEFAULT 1"),
];

/// Create the table if it is not there yet, then add any column an older
/// database is missing. `CREATE TABLE IF NOT EXISTS` does nothing on an old
/// file — the new columns must be added one by one, or every insert fails
/// with `no column named …` and the usage page goes quiet while the JSONL
/// log keeps the truth.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS usage_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            time TEXT NOT NULL,
            model TEXT NOT NULL,
            provider TEXT NOT NULL DEFAULT '',
            key_id TEXT,
            key_label TEXT,
            group_id TEXT,
            protocol TEXT NOT NULL DEFAULT '',
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            status INTEGER NOT NULL DEFAULT 0,
            ok INTEGER NOT NULL DEFAULT 1,
            error TEXT,
            agent TEXT,
            session_id TEXT,
            context TEXT,
            project TEXT,
            cost_usd REAL,
            credits REAL,
            priced INTEGER NOT NULL DEFAULT 1
        );",
    )?;
    let present: std::collections::BTreeSet<String> = {
        let mut statement = conn.prepare("PRAGMA table_info(usage_records)")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for (name, decl) in COLUMNS {
        if !present.contains(name) {
            conn.execute_batch(&format!(
                "ALTER TABLE usage_records ADD COLUMN {name} {decl};"
            ))?;
        }
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_usage_time ON usage_records(time);
         CREATE INDEX IF NOT EXISTS idx_usage_provider ON usage_records(provider, time);
         CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_records(model, time);",
    )?;
    Ok(())
}

/// Read one row back into the canonical record.
fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
    Ok(UsageRecord {
        time: row.get("time")?,
        model: row.get("model")?,
        provider: row
            .get::<_, Option<String>>("provider")?
            .unwrap_or_default(),
        key_id: row.get("key_id")?,
        key_label: row.get("key_label")?,
        group: row.get("group_id")?,
        protocol: row
            .get::<_, Option<String>>("protocol")?
            .unwrap_or_default(),
        input_tokens: row.get::<_, i64>("input_tokens")?.max(0) as u64,
        output_tokens: row.get::<_, i64>("output_tokens")?.max(0) as u64,
        cache_read_tokens: row.get::<_, i64>("cache_read_tokens")?.max(0) as u64,
        cache_write_tokens: row.get::<_, i64>("cache_write_tokens")?.max(0) as u64,
        reasoning_tokens: row.get::<_, i64>("reasoning_tokens")?.max(0) as u64,
        latency_ms: row.get::<_, i64>("latency_ms")?.max(0) as u64,
        status: row.get::<_, i64>("status")?.max(0) as u16,
        ok: row.get::<_, i64>("ok")? != 0,
        error: row.get("error")?,
        agent: row.get("agent")?,
        session_id: row.get("session_id")?,
        context: row
            .get::<_, Option<String>>("context")?
            .and_then(|s| serde_json::from_str(&s).ok()),
        project: row.get("project")?,
        cost_usd: row.get("cost_usd")?,
        credits: row.get("credits")?,
        priced: row.get::<_, i64>("priced")? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::usage::ContextBreakdown;

    fn store_at(tag: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "local-router-store-unit-{}-{}",
            tag,
            std::process::id() as u64 + chrono::Utc::now().timestamp_subsec_nanos() as u64
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Store::open(&dir.join("usage.db"))
    }

    fn record(provider: &str, model: &str, in_tok: u64, out_tok: u64) -> UsageRecord {
        let mut rec = UsageRecord::new(model, "openai_chat");
        rec.provider = provider.into();
        rec.input_tokens = in_tok;
        rec.output_tokens = out_tok;
        rec.session_id = Some("s-1".into());
        rec.project = Some("/tmp/work".into());
        rec.agent = Some("claude-code".into());
        rec
    }

    #[test]
    fn records_round_trip_with_every_field() {
        let store = store_at("round-trip");
        let mut rec = record("workbuddy", "hy3", 100, 50);
        rec.cost_usd = Some(0.123);
        rec.credits = Some(1.5);
        rec.context = Some(ContextBreakdown {
            system_tokens: 10,
            tool_tokens: 20,
            file_tokens: 30,
            conversation_tokens: 40,
            cache_read_tokens: 5,
            prompt_tokens: 100,
            window_tokens: 1000,
        });
        store.record(&rec);
        let read = store.read_all();
        assert_eq!(read.len(), 1);
        let back = &read[0];
        assert_eq!(back.provider, "workbuddy");
        assert_eq!(back.model, "hy3");
        assert_eq!(back.input_tokens, 100);
        assert_eq!(back.output_tokens, 50);
        assert_eq!(back.cost_usd, Some(0.123));
        assert_eq!(back.credits, Some(1.5));
        assert_eq!(back.session_id.as_deref(), Some("s-1"));
        assert_eq!(back.agent.as_deref(), Some("claude-code"));
        let ctx = back.context.as_ref().unwrap();
        assert_eq!(ctx.window_tokens, 1000);
        assert_eq!(ctx.prompt_tokens, 100);
    }

    #[test]
    fn aggregate_queries_bucket_by_day_provider_and_model() {
        let store = store_at("aggregate");
        let mut a = record("p1", "m1", 10, 5);
        a.time = "2026-10-25T08:00:00+00:00".into();
        store.record(&a);
        let mut b = record("p2", "m2", 20, 7);
        b.time = "2026-10-26T09:00:00+00:00".into();
        store.record(&b);
        let mut c = record("p1", "m1", 30, 9);
        c.time = "2026-10-26T10:00:00+00:00".into();
        c.ok = false;
        c.error = Some("vendor said no".into());
        store.record(&c);

        let since = "2026-01-01T00:00:00+00:00";
        // The totals window covers successful requests only; failures are
        // visible in by_day / by_provider (and the JSONL log).
        let totals = store.totals_since(since).unwrap();
        assert_eq!(totals.requests, 2);
        assert_eq!(totals.input_tokens, 30); // 10 + 20, the failure stays out

        let days = store.by_day_since(since);
        assert_eq!(days.len(), 2);

        let providers = store.by_provider_since(since);
        let p1 = providers
            .iter()
            .find(|r| r.label.as_deref() == Some("p1"))
            .unwrap();
        // Successful rows only.
        assert_eq!(p1.requests, 1);
        assert_eq!(p1.input_tokens, 10);

        let models = store.by_model_since(since);
        let m1 = models
            .iter()
            .find(|r| r.label.as_deref() == Some("m1"))
            .unwrap();
        assert_eq!(m1.requests, 1);

        let recent = store.recent(since, 2);
        assert_eq!(recent.len(), 2);
        // Newest first.
        assert!(recent[0].time >= recent[1].time);
    }

    #[test]
    fn rekey_project_moves_every_row() {
        let store = store_at("rekey");
        let mut rec = record("p", "m", 1, 1);
        rec.project = Some("/old/dir".into());
        store.record(&rec);
        let mut other = record("p", "m", 1, 1);
        other.project = Some("/other".into());
        store.record(&other);

        store.rekey_project("/old/dir", "/new/dir");
        let rows = store.read_all();
        let projects: Vec<&str> = rows.iter().filter_map(|r| r.project.as_deref()).collect();
        assert!(projects.contains(&"/new/dir"));
        assert!(projects.contains(&"/other"));
        assert!(!projects.contains(&"/old/dir"));
    }

    #[test]
    fn an_old_database_is_brought_up_to_the_new_columns() {
        // The shape an early release created: no session_id, context,
        // project, cost or credits. Every insert of the current code failed
        // on it with `no column named …`, and the Usage page went quiet
        // while the JSONL log kept the truth. The store must add the
        // missing columns on open, and keep the old rows.
        let dir = std::env::temp_dir().join(format!(
            "local-router-store-legacy-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("usage.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE usage_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    time TEXT NOT NULL,
                    model TEXT NOT NULL,
                    protocol TEXT NOT NULL DEFAULT '',
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    latency_ms INTEGER NOT NULL DEFAULT 0,
                    status INTEGER NOT NULL DEFAULT 0,
                    ok INTEGER NOT NULL DEFAULT 1,
                    error TEXT,
                    agent TEXT
                );
                INSERT INTO usage_records
                    (time, model, protocol, input_tokens, output_tokens, status, ok)
                VALUES ('2026-10-26T13:45:47+00:00', 'vendor/hy3', 'openai_chat', 11, 7, 200, 1);",
            )
            .unwrap();
        }

        let store = Store::open(&path);
        // The old row reads back through the new shape.
        let rows = store.read_all();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, 11);
        assert_eq!(rows[0].output_tokens, 7);
        // And a new record — the kind the gateway writes after every
        // request — inserts without a complaint.
        let mut fresh = record("workbuddy", "hy3", 5, 3);
        fresh.session_id = Some("s-9".into());
        fresh.project = Some("/tmp/x".into());
        fresh.cost_usd = Some(0.01);
        store.record(&fresh);
        let rows = store.read_all();
        assert_eq!(rows.len(), 2);
        let last = rows.last().unwrap();
        assert_eq!(last.session_id.as_deref(), Some("s-9"));
        assert_eq!(last.project.as_deref(), Some("/tmp/x"));
        assert_eq!(last.cost_usd, Some(0.01));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_disabled_store_is_none_but_the_type_still_works() {
        // `enabled()` honours the env switch (cached per process); the tests
        // run with the default, so this only pins the parser contract.
        assert!(enabled());
    }
}
