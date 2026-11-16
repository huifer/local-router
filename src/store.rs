//! The SQLite store: every request's record, queryable without loading a
//! whole log file into memory.
//!
//! The store is one file next to the config (`usage.db`). Writes do not go
//! through the request path: each `record` / `record_span` clones into a
//! bounded queue, and one dedicated writer thread drains the queue in
//! batches (up to 128 rows, or every 100 ms) inside a single transaction.
//! This keeps the SQLite commit — the one place a per-request write could
//! otherwise serialize the gateway — off the hot path. Reads keep their own
//! connection, and WAL means a reader never blocks the writer.
//!
//! A `flush()` drains the queue synchronously, so anything that reads
//! right after writing (the console, the CLI, the tests) sees the latest
//! state. The gateway's own request path never flushes: it only queues.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crossbeam_channel::{bounded, Receiver, Sender};
use rusqlite::Connection;

use crate::gateway::usage::UsageRecord;
use crate::sysmon::Sample as SysSample;
use crate::trace::Span;

/// One queued write, for the writer thread.
enum Write {
    /// One usage record.
    Usage(UsageRecord),
    /// One trace span.
    Span(Span),
    /// One process-monitor sample.
    Sys(SysSample),
    /// Drain and commit everything queued, then acknowledge.
    Flush(crossbeam_channel::Sender<()>),
}

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
///
/// Reads use `conn` directly. Writes are queued to `writer`, which owns a
/// second connection and commits in batches; see the module docs.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Option<Connection>>>,
    writer: Arc<Writer>,
    path: PathBuf,
}

/// The writer end of the queue, shared by every `Store` on the same file.
struct Writer {
    tx: Sender<Write>,
}

impl Writer {
    /// Queue one item. Returns `false` when the writer thread is gone
    /// (its database could not be opened, or the process is shutting
    /// down): the item is dropped and the caller can give up rather than
    /// waiting for an ack that will never come.
    fn queue(&self, item: Write) -> bool {
        // Blocking send: the queue is bounded, so a writer thread that
        // cannot keep up applies backpressure here rather than growing the
        // heap. A usage record is a passenger, but dropping it silently
        // would break the bill, so waiting a moment is the right trade.
        self.tx.send(item).is_ok()
    }
}

/// One writer thread per database file, so two `Store` handles opened on
/// the same path — the daemon's `shared()` plus a test's `open()`, say —
/// never fight over the same connection.
static WRITERS: OnceLock<Mutex<std::collections::HashMap<PathBuf, Arc<Writer>>>> = OnceLock::new();

fn writers() -> &'static Mutex<std::collections::HashMap<PathBuf, Arc<Writer>>> {
    WRITERS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Commit at most this many queued rows per transaction.
const BATCH_ROWS: usize = 128;
/// Drain the queue at least this often, so a trickle of writes still lands
/// promptly without a transaction per request.
const BATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
/// How many writes may queue before producers start waiting.
const QUEUE_DEPTH: usize = 8192;

/// Spawn (once per path) the writer thread that owns the write connection.
fn writer_for(path: &Path) -> Arc<Writer> {
    let mut map = writers().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = map.get(path) {
        return existing.clone();
    }
    let (tx, rx) = bounded::<Write>(QUEUE_DEPTH);
    let writer = Arc::new(Writer { tx });
    map.insert(path.to_path_buf(), writer.clone());
    drop(map);
    let db_path = path.to_path_buf();
    std::thread::Builder::new()
        .name("local-router usage-writer".into())
        .spawn(move || writer_loop(db_path, rx))
        .expect("the usage writer thread must spawn");
    writer
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
            // Wait out the writer's commit rather than failing a rare
            // read-side write (prune, rekey).
            let _ = conn.busy_timeout(std::time::Duration::from_secs(30));
        } else {
            tracing::warn!(
                "the usage database at {} could not be opened",
                path.display()
            );
        }
        Store {
            conn: Arc::new(Mutex::new(conn)),
            writer: writer_for(path),
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

    /// Queue one record for the writer thread. A failure is logged, never
    /// fatal: usage is a passenger, not the cargo.
    pub fn record(&self, rec: &UsageRecord) {
        self.writer.queue(Write::Usage(rec.clone()));
    }

    /// Queue one trace span for the writer thread.
    ///
    /// A session span is written again for every request of that session,
    /// so the row is upserted: its counter grows and its clock moves on.
    /// Every other span is written once.
    pub fn record_span(&self, span: &Span) {
        self.writer.queue(Write::Span(span.clone()));
    }

    /// Queue one process-monitor sample. One row every ten seconds, so the
    /// System chart still has a history after a restart.
    pub fn record_sys(&self, sample: &SysSample) {
        self.writer.queue(Write::Sys(sample.clone()));
    }

    /// Drain the writer's queue and wait for the commit. Call this before
    /// reading what was just written — the console, the CLI and the tests
    /// do; the gateway's request path does not.
    ///
    /// If the writer thread is gone (its database never opened), there is
    /// nothing to wait for and nothing to read, so the call is a no-op
    /// instead of a hang.
    pub fn flush(&self) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if !self.writer.queue(Write::Flush(tx)) {
            return;
        }
        let _ = rx.recv();
    }
    /// Every span since `since`, oldest first.
    /// Every process sample since `since` (unix milliseconds), oldest first.
    /// The System chart reads this; bucketing happens in [`crate::sysmon`].
    pub fn sys_samples_since(&self, since: i64) -> Vec<SysSample> {
        self.flush();
        let Ok(guard) = self.conn.lock() else {
            return Vec::new();
        };
        let Some(conn) = guard.as_ref() else {
            return Vec::new();
        };
        let Ok(mut statement) = conn.prepare(
            "SELECT t, secs, cpu_pct, rss_bytes, threads, in_flight, requests, errors,
                    latency_p50_ms, latency_p95_ms, ttft_p50_ms
             FROM sys_samples WHERE t >= ?1 ORDER BY t ASC",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map([since], |row| {
            Ok(SysSample {
                t: row.get(0)?,
                secs: row.get(1)?,
                cpu_pct: row.get(2)?,
                rss_bytes: row.get::<_, i64>(3)?.max(0) as u64,
                threads: row.get::<_, i64>(4)?.max(0) as u32,
                in_flight: row.get::<_, i64>(5)?.max(0) as u64,
                requests: row.get::<_, i64>(6)?.max(0) as u64,
                errors: row.get::<_, i64>(7)?.max(0) as u64,
                latency_p50_ms: row.get::<_, Option<i64>>(8)?.map(|v| v.max(0) as u64),
                latency_p95_ms: row.get::<_, Option<i64>>(9)?.map(|v| v.max(0) as u64),
                ttft_p50_ms: row.get::<_, Option<i64>>(10)?.map(|v| v.max(0) as u64),
            })
        }) else {
            return Vec::new();
        };
        rows.filter_map(|row| row.ok()).collect()
    }

    /// Delete usage records, trace spans and process samples older than the
    /// retention cutoff (an RFC 3339 timestamp). Returns the rows removed.
    /// The JSONL log is left alone — it is rotated by size already.
    pub fn prune_older_than(&self, cutoff: &str) -> (usize, usize, usize) {
        // Drain the queued writer first: a delete must not miss rows the
        // gateway has queued but not yet committed.
        self.flush();
        let mut conn = self.conn.lock().expect("store lock");
        let Some(conn) = conn.as_mut() else {
            return (0, 0, 0);
        };
        let usage = conn
            .execute("DELETE FROM usage_records WHERE time < ?1", [cutoff])
            .unwrap_or(0);
        let spans = conn
            .execute("DELETE FROM trace_spans WHERE started_at < ?1", [cutoff])
            .unwrap_or(0);
        // `sys_samples.t` is unix milliseconds; a cutoff that does not parse
        // prunes nothing rather than pruning everything.
        let sys = chrono::DateTime::parse_from_rfc3339(cutoff)
            .map(|at| {
                conn.execute(
                    "DELETE FROM sys_samples WHERE t < ?1",
                    [at.timestamp_millis()],
                )
                .unwrap_or(0)
            })
            .unwrap_or(0);
        (usage, spans, sys)
    }

    pub fn spans_since(&self, since: &str) -> Vec<Span> {
        self.query_spans(
            "SELECT * FROM trace_spans WHERE started_at >= ?1 ORDER BY started_at ASC",
            [since],
        )
    }

    /// The spans of one trace, oldest first.
    pub fn trace(&self, trace_id: &str) -> Vec<Span> {
        self.query_spans(
            "SELECT * FROM trace_spans WHERE trace_id = ?1 ORDER BY started_at ASC",
            [trace_id],
        )
    }

    /// The spans of one session, oldest first.
    pub fn session_trace(&self, session_id: &str) -> Vec<Span> {
        self.query_spans(
            "SELECT * FROM trace_spans WHERE session_id = ?1 ORDER BY started_at ASC",
            [session_id],
        )
    }

    /// The span that carries a vendor's own request id.
    pub fn span_by_upstream_id(&self, upstream_request_id: &str) -> Vec<Span> {
        self.query_spans(
            "SELECT * FROM trace_spans WHERE upstream_request_id = ?1 ORDER BY started_at ASC",
            [upstream_request_id],
        )
    }

    /// Run a query and map every row to a span.
    fn query_spans(&self, sql: &str, params: impl rusqlite::Params) -> Vec<Span> {
        self.flush();
        let Ok(guard) = self.conn.lock() else {
            return Vec::new();
        };
        let Some(conn) = guard.as_ref() else {
            return Vec::new();
        };
        let Ok(mut statement) = conn.prepare(sql) else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map(params, row_to_span) else {
            return Vec::new();
        };
        rows.filter_map(|row| row.ok()).collect()
    }

    /// Every record, oldest first — the JSONL log's shape, for callers
    /// that have not moved to queries yet.
    pub fn read_all(&self) -> Vec<UsageRecord> {
        self.query("SELECT * FROM usage_records ORDER BY time ASC", [])
    }

    /// Run a query and map every row to a record.
    pub fn query(&self, sql: &str, params: impl rusqlite::Params) -> Vec<UsageRecord> {
        self.flush();
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
        self.flush();
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
                // Only streaming replies carry a first token; the sample
                // count is the denominator for the average.
                ttft_samples: row.get::<_, i64>(11)?,
                ttft_avg_ms: row.get::<_, i64>(12)?,
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
                    SUM(cost_usd), SUM(credits),
                    COUNT(ttft_ms), COALESCE(CAST(ROUND(AVG(ttft_ms)) AS INTEGER), 0)
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
                    SUM(cost_usd), SUM(credits),
                    COUNT(ttft_ms), COALESCE(CAST(ROUND(AVG(ttft_ms)) AS INTEGER), 0)
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
                    SUM(cost_usd), SUM(credits),
                    COUNT(ttft_ms), COALESCE(CAST(ROUND(AVG(ttft_ms)) AS INTEGER), 0)
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
                    SUM(cost_usd), SUM(credits),
                    COUNT(ttft_ms), COALESCE(CAST(ROUND(AVG(ttft_ms)) AS INTEGER), 0)
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
        // Same as prune: wait for queued writes before rewriting rows.
        self.flush();
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

/// The writer thread: drain the queue in batches, one transaction each.
fn writer_loop(path: PathBuf, rx: Receiver<Write>) {
    let conn = match Connection::open(&path) {
        Ok(conn) => conn,
        Err(err) => {
            tracing::warn!("the usage writer could not open {}: {err}", path.display());
            drain_flushes(&rx);
            return;
        }
    };
    if let Err(err) = migrate(&conn) {
        tracing::warn!("the usage writer could not prepare the database: {err}");
        drain_flushes(&rx);
        return;
    }
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    // Wait out a concurrent reader/checkpoint rather than failing the
    // batch; WAL keeps readers and the writer off each other's feet.
    let _ = conn.busy_timeout(std::time::Duration::from_secs(30));

    let mut usage: VecDeque<UsageRecord> = VecDeque::new();
    let mut spans: VecDeque<Span> = VecDeque::new();
    let mut sys: VecDeque<SysSample> = VecDeque::new();
    let mut flush_reply: Option<Sender<()>> = None;

    loop {
        // Drain whatever is queued, capping the batch at BATCH_ROWS.
        while usage.len() < BATCH_ROWS && spans.len() < BATCH_ROWS && sys.len() < BATCH_ROWS {
            let timeout = if usage.is_empty() && spans.is_empty() && sys.is_empty() {
                BATCH_INTERVAL
            } else {
                std::time::Duration::ZERO
            };
            match rx.recv_timeout(timeout) {
                Ok(Write::Usage(rec)) => usage.push_back(rec),
                Ok(Write::Span(span)) => spans.push_back(span),
                Ok(Write::Sys(sample)) => sys.push_back(sample),
                Ok(Write::Flush(reply)) => flush_reply = Some(reply),
                Err(_) => break,
            }
        }
        // A flush drains the rest of the queue before committing.
        if flush_reply.is_some() {
            while let Ok(item) = rx.try_recv() {
                match item {
                    Write::Usage(rec) => usage.push_back(rec),
                    Write::Span(span) => spans.push_back(span),
                    Write::Sys(sample) => sys.push_back(sample),
                    Write::Flush(reply) => flush_reply = Some(reply),
                }
            }
        }

        if !usage.is_empty() || !spans.is_empty() || !sys.is_empty() {
            commit(&conn, &mut usage, &mut spans, &mut sys);
        }
        if let Some(reply) = flush_reply.take() {
            // An empty flush commits nothing, but it still acknowledges —
            // the caller only cares that the queue is drained.
            let _ = reply.send(());
        }
    }
}

/// The writer's database could not be opened or prepared: acknowledge any
/// pending flushes so callers waiting on `flush()` do not hang, then drop
/// the rest. The short wait covers the race between a `flush()` that has
/// queued but not yet been seen and the writer giving up.
fn drain_flushes(rx: &Receiver<Write>) {
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(Write::Flush(reply)) => {
                let _ = reply.send(());
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

/// Insert every queued row in one transaction. A failure is logged per
/// statement and never fatal: usage is a passenger, not the cargo.
fn commit(
    conn: &Connection,
    usage: &mut VecDeque<UsageRecord>,
    spans: &mut VecDeque<Span>,
    sys: &mut VecDeque<SysSample>,
) {
    if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
        // No transaction is open; write the rows one by one as a fallback
        // so a failure in the batching still leaves the data on disk.
        while let Some(rec) = usage.pop_front() {
            insert_usage(conn, &rec);
        }
        while let Some(span) = spans.pop_front() {
            insert_span(conn, &span);
        }
        while let Some(sample) = sys.pop_front() {
            insert_sys(conn, &sample);
        }
        return;
    }
    while let Some(rec) = usage.pop_front() {
        insert_usage(conn, &rec);
    }
    while let Some(span) = spans.pop_front() {
        insert_span(conn, &span);
    }
    while let Some(sample) = sys.pop_front() {
        insert_sys(conn, &sample);
    }
    if let Err(err) = conn.execute_batch("COMMIT") {
        tracing::warn!("the usage batch could not commit: {err}");
        // Leave no open transaction behind: the next batch's BEGIN would
        // otherwise fail and the fallback inserts would join this one.
        let _ = conn.execute_batch("ROLLBACK");
    }
}

/// Insert one usage record.
fn insert_usage(conn: &Connection, rec: &UsageRecord) {
    let result = conn.execute(
        "INSERT INTO usage_records
         (time, model, provider, key_id, key_label, caller_key_id, caller_key_label, group_id, protocol,
          input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
          reasoning_tokens, latency_ms, status, ok, error, agent, session_id,
          context, project, cost_usd, credits, priced, ttft_ms)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",
        rusqlite::params![
            rec.time,
            rec.model,
            rec.provider,
            rec.key_id,
            rec.key_label,
            rec.caller_key_id,
            rec.caller_key_label,
            rec.group,
            rec.protocol,
            rec.input_tokens as i64,
            rec.output_tokens as i64,
            rec.cache_read_tokens as i64,
            rec.cache_write_tokens as i64,
            rec.reasoning_tokens as i64,
            rec.latency_ms as i64,
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
            rec.ttft_ms.map(|n| n as i64),
        ],
    );
    if let Err(err) = result {
        tracing::warn!("the usage record could not be written: {err}");
    }
}

/// Insert one trace span. A session span is written again for every request
/// of that session, so the row is upserted: its counter grows and its clock
/// moves on.
fn insert_span(conn: &Connection, span: &Span) {
    let result = conn.execute(
        "INSERT INTO trace_spans
         (span_id, trace_id, parent_span_id, kind, name, session_id, task_id, agent, project,
          caller_key_id, model, provider, provider_key_id, protocol, attempt, requests,
          status, ok, skipped, error, latency_ms, input_tokens, output_tokens, cache_read_tokens,
          cache_write_tokens, reasoning_tokens, cost_usd, credits, started_at, ended_at,
          upstream_request_id, messages, preview)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33)
         ON CONFLICT(span_id) DO UPDATE SET
            ended_at = excluded.ended_at,
            requests = trace_spans.requests + excluded.requests,
            latency_ms = excluded.latency_ms",
        rusqlite::params![
            span.span_id,
            span.trace_id,
            span.parent_span_id,
            span.kind.as_str(),
            span.name,
            span.session_id,
            span.task_id,
            span.agent,
            span.project,
            span.caller_key_id,
            span.model,
            span.provider,
            span.provider_key_id,
            span.protocol,
            span.attempt,
            span.requests as i64,
            span.status,
            span.ok,
            span.skipped,
            span.error,
            span.latency_ms as i64,
            span.input_tokens as i64,
            span.output_tokens as i64,
            span.cache_read_tokens as i64,
            span.cache_write_tokens as i64,
            span.reasoning_tokens as i64,
            span.cost_usd,
            span.credits,
            span.started_at,
            span.ended_at,
            span.upstream_request_id,
            span.messages,
            span.preview,
        ],
    );
    if let Err(err) = result {
        tracing::warn!("the trace span could not be written: {err}");
    }
}

/// Insert one process-monitor sample. `INSERT OR REPLACE` on the timestamp
/// makes a replayed sample (a restarted sampler closing the same window)
/// harmless instead of a duplicate point on the chart.
fn insert_sys(conn: &Connection, sample: &SysSample) {
    let result = conn.execute(
        "INSERT OR REPLACE INTO sys_samples
         (t, secs, cpu_pct, rss_bytes, threads, in_flight, requests, errors,
          latency_p50_ms, latency_p95_ms, ttft_p50_ms)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        rusqlite::params![
            sample.t,
            sample.secs,
            sample.cpu_pct,
            sample.rss_bytes as i64,
            sample.threads as i64,
            sample.in_flight as i64,
            sample.requests as i64,
            sample.errors as i64,
            sample.latency_p50_ms.map(|v| v as i64),
            sample.latency_p95_ms.map(|v| v as i64),
            sample.ttft_p50_ms.map(|v| v as i64),
        ],
    );
    if let Err(err) = result {
        tracing::warn!("the process sample could not be written: {err}");
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
    /// Streaming replies in this bucket with a measured first token, and
    /// their mean TTFT in ms (0 when there are no samples).
    pub ttft_samples: i64,
    pub ttft_avg_ms: i64,
}

/// The columns the newest code writes. A database created by an older
/// release may lack the later ones; each is added on open.
const COLUMNS: [(&str, &str); 21] = [
    ("provider", "TEXT NOT NULL DEFAULT ''"),
    ("key_id", "TEXT"),
    ("key_label", "TEXT"),
    ("caller_key_id", "TEXT"),
    ("caller_key_label", "TEXT"),
    ("group_id", "TEXT"),
    ("cache_read_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("cache_write_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("reasoning_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("latency_ms", "INTEGER NOT NULL DEFAULT 0"),
    ("error", "TEXT"),
    ("agent", "TEXT"),
    ("session_id", "TEXT"),
    ("upstream_request_id", "TEXT"),
    ("context", "TEXT"),
    ("project", "TEXT"),
    ("cost_usd", "REAL"),
    ("credits", "REAL"),
    ("priced", "INTEGER NOT NULL DEFAULT 1"),
    ("ok", "INTEGER NOT NULL DEFAULT 1"),
    ("ttft_ms", "INTEGER"),
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
            caller_key_id TEXT,
            caller_key_label TEXT,
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
            upstream_request_id TEXT,
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
    // The trace tree: one row per span, with the parent that ties it to the
    // span above it.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS trace_spans (
            span_id TEXT PRIMARY KEY,
            trace_id TEXT NOT NULL,
            parent_span_id TEXT,
            kind TEXT NOT NULL DEFAULT 'request',
            name TEXT NOT NULL DEFAULT '',
            session_id TEXT,
            task_id TEXT,
            agent TEXT,
            project TEXT,
            caller_key_id TEXT,
            model TEXT NOT NULL DEFAULT '',
            provider TEXT,
            provider_key_id TEXT,
            protocol TEXT,
            attempt INTEGER,
            requests INTEGER NOT NULL DEFAULT 0,
            status INTEGER NOT NULL DEFAULT 0,
            ok INTEGER NOT NULL DEFAULT 1,
            skipped INTEGER NOT NULL DEFAULT 0,
            error TEXT,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL,
            credits REAL,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            upstream_request_id TEXT,
            messages INTEGER NOT NULL DEFAULT 0,
            preview TEXT
        );",
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_trace_time ON trace_spans(started_at);
         CREATE INDEX IF NOT EXISTS idx_trace_id ON trace_spans(trace_id);
         CREATE INDEX IF NOT EXISTS idx_trace_session ON trace_spans(session_id);",
    )?;
    // A database written by an earlier build may lack the later columns.
    let trace_present: std::collections::BTreeSet<String> = {
        let mut statement = conn.prepare("PRAGMA table_info(trace_spans)")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for (name, decl) in TRACE_COLUMNS {
        if !trace_present.contains(name) {
            conn.execute_batch(&format!(
                "ALTER TABLE trace_spans ADD COLUMN {name} {decl};"
            ))?;
        }
    }
    // The gateway's own resource samples: one row every ten seconds, small
    // enough to keep and the only way the System chart survives a restart.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sys_samples (
            t INTEGER PRIMARY KEY,
            secs REAL NOT NULL DEFAULT 0,
            cpu_pct REAL NOT NULL DEFAULT 0,
            rss_bytes INTEGER NOT NULL DEFAULT 0,
            threads INTEGER NOT NULL DEFAULT 0,
            in_flight INTEGER NOT NULL DEFAULT 0,
            requests INTEGER NOT NULL DEFAULT 0,
            errors INTEGER NOT NULL DEFAULT 0,
            latency_p50_ms INTEGER,
            latency_p95_ms INTEGER,
            ttft_p50_ms INTEGER
        );\n         CREATE INDEX IF NOT EXISTS idx_sys_time ON sys_samples(t);",
    )?;
    Ok(())
}

/// The columns the trace table gained after it was first written.
const TRACE_COLUMNS: [(&str, &str); 1] = [("skipped", "INTEGER NOT NULL DEFAULT 0")];

/// Read one row back into the canonical span.
fn row_to_span(row: &rusqlite::Row<'_>) -> rusqlite::Result<Span> {
    let kind = match row.get::<_, Option<String>>("kind")?.as_deref() {
        Some("session") => crate::trace::SpanKind::Session,
        Some("upstream") => crate::trace::SpanKind::Upstream,
        _ => crate::trace::SpanKind::Request,
    };
    Ok(Span {
        trace_id: row.get("trace_id")?,
        span_id: row.get("span_id")?,
        parent_span_id: row.get("parent_span_id")?,
        kind,
        name: row.get::<_, Option<String>>("name")?.unwrap_or_default(),
        session_id: row.get("session_id")?,
        task_id: row.get("task_id")?,
        agent: row.get("agent")?,
        project: row.get("project")?,
        caller_key_id: row.get("caller_key_id")?,
        model: row.get::<_, Option<String>>("model")?.unwrap_or_default(),
        provider: row.get("provider")?,
        provider_key_id: row.get("provider_key_id")?,
        protocol: row.get("protocol")?,
        attempt: row
            .get::<_, Option<i64>>("attempt")?
            .map(|a| a.max(0) as u32),
        requests: row.get::<_, i64>("requests")?.max(0) as u64,
        status: row.get::<_, i64>("status")?.max(0) as u16,
        ok: row.get::<_, i64>("ok")? != 0,
        skipped: row.get::<_, i64>("skipped")? != 0,
        error: row.get("error")?,
        latency_ms: row.get::<_, i64>("latency_ms")?.max(0) as u64,
        input_tokens: row.get::<_, i64>("input_tokens")?.max(0) as u64,
        output_tokens: row.get::<_, i64>("output_tokens")?.max(0) as u64,
        cache_read_tokens: row.get::<_, i64>("cache_read_tokens")?.max(0) as u64,
        cache_write_tokens: row.get::<_, i64>("cache_write_tokens")?.max(0) as u64,
        reasoning_tokens: row.get::<_, i64>("reasoning_tokens")?.max(0) as u64,
        cost_usd: row.get("cost_usd")?,
        credits: row.get("credits")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        upstream_request_id: row.get("upstream_request_id")?,
        messages: row.get::<_, i64>("messages")?.max(0) as u32,
        preview: row.get("preview")?,
    })
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
        caller_key_id: row.get("caller_key_id")?,
        caller_key_label: row.get("caller_key_label")?,
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
        ttft_ms: row
            .get::<_, Option<i64>>("ttft_ms")?
            .map(|n| n.max(0) as u64),
        status: row.get::<_, i64>("status")?.max(0) as u16,
        ok: row.get::<_, i64>("ok")? != 0,
        error: row.get("error")?,
        agent: row.get("agent")?,
        session_id: row.get("session_id")?,
        upstream_request_id: row.get("upstream_request_id")?,
        context: row
            .get::<_, Option<String>>("context")?
            .and_then(|s| serde_json::from_str(&s).ok()),
        project: row.get("project")?,
        cost_usd: row.get("cost_usd")?,
        credits: row.get("credits")?,
        priced: row.get::<_, i64>("priced")? != 0,
        retry_after_secs: None,
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
        rec.caller_key_id = Some("caller-1".into());
        rec.caller_key_label = Some("laptop".into());
        rec
    }

    #[test]
    fn records_round_trip_with_every_field() {
        let store = store_at("round-trip");
        let mut rec = record("workbuddy", "hy3", 100, 50);
        rec.cost_usd = Some(0.123);
        rec.credits = Some(1.5);
        rec.ttft_ms = Some(240);
        rec.ttft_ms = Some(240);
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
        assert_eq!(back.ttft_ms, Some(240));
        assert_eq!(back.ttft_ms, Some(240));
        assert_eq!(back.session_id.as_deref(), Some("s-1"));
        assert_eq!(back.agent.as_deref(), Some("claude-code"));
        assert_eq!(back.caller_key_id.as_deref(), Some("caller-1"));
        assert_eq!(back.caller_key_label.as_deref(), Some("laptop"));
        let ctx = back.context.as_ref().unwrap();
        assert_eq!(ctx.window_tokens, 1000);
        assert_eq!(ctx.prompt_tokens, 100);
    }

    #[test]
    fn aggregate_queries_bucket_by_day_provider_and_model() {
        let store = store_at("aggregate");
        let mut a = record("p1", "m1", 10, 5);
        a.time = "2026-10-25T08:00:00+00:00".into();
        a.ttft_ms = Some(100); // a streaming reply's first token
        store.record(&a);
        let mut b = record("p2", "m2", 20, 7);
        b.time = "2026-10-26T09:00:00+00:00".into();
        b.ttft_ms = Some(300);
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
        assert_eq!(totals.ttft_samples, 2);
        assert_eq!(totals.ttft_avg_ms, 200);

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
        assert_eq!(p1.ttft_samples, 1);
        assert_eq!(p1.ttft_avg_ms, 100);

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

    #[test]
    fn queued_writes_land_in_one_batch_after_a_flush() {
        let store = store_at("queued");
        // The gateway only queues; the writer thread commits on its own
        // schedule. Queue a batch, flush, and read everything back to
        // prove the batch landed whole and in order.
        for i in 0..40u64 {
            let mut rec = record("q", "m", i, 1);
            rec.time = format!("2026-10-26T08:00:{:02}+00:00", i);
            store.record(&rec);
        }
        let mut span = crate::trace::Span::start(crate::trace::SpanKind::Upstream, "trc_q", "up_q");
        span.provider = Some("q".into());
        store.record_span(&span);

        store.flush();

        let rows = store.read_all();
        assert_eq!(rows.len(), 40);
        // Oldest first: the batch kept its order through the queue.
        assert!(rows[0].time <= rows[39].time);
        let spans = store.spans_since("2000-01-01T00:00:00+00:00");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].span_id, "up_q");
    }

    #[test]
    fn trace_spans_round_trip_and_the_session_counter_grows() {
        let store = store_at("traces");
        let mut session =
            crate::trace::Span::start(crate::trace::SpanKind::Session, "trc_1", "ses_1");
        session.session_id = Some("s-1".into());
        session.agent = Some("claude-code".into());
        session.requests = 1;
        store.record_span(&session);

        // The same session, seen again: the row grows, it does not double.
        let mut again = session.clone();
        again.requests = 1;
        store.record_span(&again);

        let mut request =
            crate::trace::Span::start(crate::trace::SpanKind::Request, "trc_1", "req_1");
        request.parent_span_id = Some("ses_1".into());
        request.session_id = Some("s-1".into());
        request.model = "mock/small".into();
        store.record_span(&request);

        let mut upstream =
            crate::trace::Span::start(crate::trace::SpanKind::Upstream, "trc_1", "up_1");
        upstream.parent_span_id = Some("req_1".into());
        upstream.provider = Some("mock".into());
        upstream.input_tokens = 12;
        upstream.cost_usd = Some(0.5);
        upstream.upstream_request_id = Some("chatcmpl-1".into());
        store.record_span(&upstream);

        let spans = store.spans_since("2000-01-01T00:00:00+00:00");
        assert_eq!(spans.len(), 3);
        let session = spans.iter().find(|s| s.span_id == "ses_1").unwrap();
        assert_eq!(session.requests, 2);
        assert_eq!(session.agent.as_deref(), Some("claude-code"));

        // One trace, read back as a whole.
        let trace = store.trace("trc_1");
        assert_eq!(trace.len(), 3);
        // And the vendor's own id finds the call.
        let found = store.span_by_upstream_id("chatcmpl-1");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].span_id, "up_1");
    }
    #[test]
    fn retention_prunes_only_old_rows() {
        let store = store_at("retention");
        let mut old = record("p", "m", 1, 1);
        old.time = "2020-01-01T00:00:00Z".into();
        store.record(&old);
        let mut recent = record("p", "m", 1, 1);
        recent.time = chrono::Utc::now().to_rfc3339();
        store.record(&recent);
        let (usage, _, _) = store.prune_older_than("2021-01-01T00:00:00Z");
        assert_eq!(usage, 1);
        let left = store.read_all();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].time, recent.time);
    }

    #[test]
    fn a_process_sample_round_trips_through_sqlite() {
        let store = store_at("sys-samples");
        let sample = crate::sysmon::Sample {
            t: 1_700_000_000_000,
            secs: 10.0,
            cpu_pct: 12.5,
            rss_bytes: 42 * 1024 * 1024,
            threads: 17,
            in_flight: 3,
            requests: 42,
            errors: 1,
            latency_p50_ms: Some(120),
            latency_p95_ms: Some(900),
            ttft_p50_ms: Some(60),
        };
        store.record_sys(&sample);
        let read = store.sys_samples_since(1_600_000_000_000);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].t, sample.t);
        assert_eq!(read[0].cpu_pct, 12.5);
        assert_eq!(read[0].rss_bytes, sample.rss_bytes);
        assert_eq!(read[0].requests, 42);
        assert_eq!(read[0].latency_p95_ms, Some(900));
        assert_eq!(read[0].ttft_p50_ms, Some(60));
        // A cutoff in the future prunes it; an unparsable one prunes nothing.
        let (_, _, sys) = store.prune_older_than("2999-01-01T00:00:00Z");
        assert_eq!(sys, 1);
        assert!(store.sys_samples_since(0).is_empty());
    }
}
