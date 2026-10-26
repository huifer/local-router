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
              reasoning_tokens, latency_ms, status, ok, error, agent,
              cost_usd, credits, priced)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
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
                    COUNT(*), SUM(CASE WHEN ok = 0 THEN 1 ELSE 0 END),
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

/// Create the table if it is not there yet. A missing column is added, so
/// an older database keeps working after a release adds a field.
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
            cost_usd REAL,
            credits REAL,
            priced INTEGER NOT NULL DEFAULT 1
        );
        CREATE INDEX IF NOT EXISTS idx_usage_time ON usage_records(time);
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
        cost_usd: row.get("cost_usd")?,
        credits: row.get("credits")?,
        priced: row.get::<_, i64>("priced")? != 0,
    })
}
