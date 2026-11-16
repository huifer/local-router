//! Continuous self-monitoring: the gateway's own CPU, memory, thread count,
//! and the way it is answering — request rate, failures, latency, TTFT.
//!
//! Two homes for two questions:
//!
//! - **live** — a ring of samples (one every ten seconds, six hours deep) and
//!   the Prometheus gauges. This is what `/api/system` and the console's
//!   System page read.
//! - **history** — the same samples appended to `usage.db` (`sys_samples`),
//!   pruned by the same `retention_days` as the usage ledger, so a restart
//!   does not erase the day's chart.
//!
//! CPU is reported the way `top` and `ps` report it: **percent of one core**,
//! so a process saturating two cores reads `200`. `cores` rides along in the
//! snapshot for anyone who wants to normalize.
//!
//! Latency percentiles cover *successful* replies only: a failed attempt
//! (timeout, 429, refused) is counted in `errors` but would otherwise drag
//! the "how fast does it answer" number to the length of the timeout.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::AppState;
use crate::gateway::usage::UsageRecord;
use crate::store::Store;

/// How often the sampler runs. Every chart here is at this granularity; a
/// page that polls faster simply reads the same sample again.
pub const SAMPLE_SECS: u64 = 10;

/// Six hours of live samples.
const RING_SAMPLES: usize = (6 * 60 * 60) / SAMPLE_SECS as usize;

/// Fifteen minutes of per-window histograms, for the 1m / 5m / 15m cards.
const SUMMARY_SAMPLES: usize = (15 * 60) / SAMPLE_SECS as usize;

/// The summary windows the snapshot reports, in seconds.
pub const WINDOWS: [(u64, &str); 3] = [(60, "1m"), (300, "5m"), (900, "15m")];

// ------------------------------------------------------------- histograms --

/// Log-bucketed latency histogram, in milliseconds.
///
/// Exact percentiles would mean keeping every sample; a 10-second window at
/// a few thousand requests per second is too many to keep, and the charts do
/// not need exactness. Each bucket `i` covers `[2^(i-1), 2^i)` ms, so a
/// percentile reads back at most one doubling below the true value.
const BUCKETS: usize = 32;

#[derive(Clone, Default)]
struct Hist {
    buckets: [u64; BUCKETS],
    count: u64,
    sum: u64,
}

impl Hist {
    fn add(&mut self, value: u64) {
        self.buckets[bucket_of(value)] += 1;
        self.count += 1;
        self.sum = self.sum.saturating_add(value);
    }

    fn merge(&mut self, other: &Hist) {
        for (mine, theirs) in self.buckets.iter_mut().zip(other.buckets.iter()) {
            *mine += theirs;
        }
        self.count += other.count;
        self.sum = self.sum.saturating_add(other.sum);
    }

    /// The `p`th percentile (1–100), or `None` with no samples.
    fn percentile(&self, p: u64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        // Walk up to the bucket that holds the p-th value; its lower bound is
        // the estimate.
        let target = (self.count * p).div_ceil(100);
        let mut seen = 0;
        for (index, value) in self.buckets.iter().enumerate() {
            seen += value;
            if seen >= target {
                return Some(match index {
                    0 => 0,
                    _ => 1u64 << (index - 1),
                });
            }
        }
        self.avg()
    }

    fn avg(&self) -> Option<u64> {
        (self.count > 0).then(|| self.sum / self.count)
    }
}

/// `0` lands in bucket 0, `1` in 1, `2..=3` in 2, `4..=7` in 3, and so on.
fn bucket_of(value: u64) -> usize {
    if value == 0 {
        return 0;
    }
    let bits = 64 - value.leading_zeros() as usize;
    bits.min(BUCKETS - 1)
}

// ----------------------------------------------------------------- windows --

/// One sample window's raws: what happened between two ticks.
#[derive(Clone, Default)]
struct Window {
    latency: Hist,
    ttft: Hist,
    requests: u64,
    errors: u64,
    /// How long the window really was (seconds). The sampler sets it when it
    /// closes the window, so a slept-through tick does not inflate the rate.
    secs: f64,
}

impl Window {
    fn merge(&mut self, other: &Window) {
        self.latency.merge(&other.latency);
        self.ttft.merge(&other.ttft);
        self.requests += other.requests;
        self.errors += other.errors;
        self.secs += other.secs;
    }
}

// ------------------------------------------------------------------ samples --

/// One tick of the sampler, as it is stored and charted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// When the sample closed, unix milliseconds.
    pub t: i64,
    /// The window's real length, in seconds.
    pub secs: f64,
    /// CPU used during the window, percent of one core.
    pub cpu_pct: f64,
    /// Resident set size at the end of the window.
    pub rss_bytes: u64,
    pub threads: u32,
    /// Requests being processed when the sample was taken.
    pub in_flight: u64,
    /// Requests that finished during the window.
    pub requests: u64,
    pub errors: u64,
    pub latency_p50_ms: Option<u64>,
    pub latency_p95_ms: Option<u64>,
    pub ttft_p50_ms: Option<u64>,
}

/// A sample merged into a fixed-width bucket, ready for a chart.
#[derive(Debug, Clone, Serialize)]
pub struct Point {
    pub t: i64,
    pub cpu_pct: f64,
    pub rss_bytes: u64,
    pub threads: u32,
    pub in_flight: u64,
    pub requests: u64,
    pub errors: u64,
    pub rps: f64,
    pub latency_p50_ms: Option<u64>,
    pub latency_p95_ms: Option<u64>,
    pub ttft_p50_ms: Option<u64>,
}

/// One summary window (1m / 5m / 15m) in the live snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub label: &'static str,
    pub seconds: u64,
    pub requests: u64,
    pub errors: u64,
    /// Requests per second over the window.
    pub rps: f64,
    pub latency_p50_ms: Option<u64>,
    pub latency_p95_ms: Option<u64>,
    pub latency_avg_ms: Option<u64>,
    pub ttft_p50_ms: Option<u64>,
}

/// The gateway's own state right now, plus its recent windows.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub now: i64,
    pub started: String,
    pub uptime_secs: u64,
    pub interval_secs: u64,
    /// False when the sampler is not running (a one-shot CLI, a test) — the
    /// fields below are then whatever the last sample said, or zero.
    pub sampling: bool,
    pub cores: usize,
    pub cpu_pct: f64,
    pub rss_bytes: u64,
    pub threads: u32,
    pub in_flight: u64,
    pub total_requests: u64,
    pub total_errors: u64,
    pub windows: Vec<Summary>,
}

struct Inner {
    /// The previous CPU reading, for a delta. `None` until the first tick,
    /// which is why the first tick only records the baseline.
    last_cpu: Option<(Instant, f64)>,
    ring: VecDeque<Sample>,
    windows: VecDeque<Window>,
    live: Window,
    live_started: Instant,
    total_requests: u64,
    total_errors: u64,
    cores: usize,
}

/// The process monitor. One per `AppState`; the sampler feeds it and the
/// console reads it.
pub struct Monitor {
    inner: Mutex<Inner>,
    /// The SQLite store, when it is on: samples are appended there so the
    /// chart survives a restart.
    sqlite: Option<Store>,
    started: DateTime<Utc>,
    started_instant: Instant,
    sampling: AtomicBool,
    /// True once [`spawn_sampler`] has scheduled the tick loop, so a second
    /// call (serve + a test, say) does not run two samplers.
    spawned: AtomicBool,
}

impl Monitor {
    pub fn new(sqlite: Option<Store>) -> Self {
        let now = Instant::now();
        Monitor {
            inner: Mutex::new(Inner {
                last_cpu: None,
                ring: VecDeque::new(),
                windows: VecDeque::new(),
                live: Window::default(),
                live_started: now,
                total_requests: 0,
                total_errors: 0,
                cores: std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1),
            }),
            sqlite,
            started: Utc::now(),
            started_instant: now,
            sampling: AtomicBool::new(false),
            spawned: AtomicBool::new(false),
        }
    }

    /// Note one finished request. Fed from the same places that feed
    /// `/metrics`, so the System page counts what the ledger records.
    pub fn record(&self, rec: &UsageRecord) {
        let mut inner = self.lock();
        inner.live.requests += 1;
        inner.total_requests += 1;
        if rec.ok {
            // Only successful replies feed the latency percentiles: a
            // timeout would otherwise be the slowest "answer" on the chart.
            inner.live.latency.add(rec.latency_ms);
            if let Some(ttft) = rec.ttft_ms {
                inner.live.ttft.add(ttft);
            }
        } else {
            inner.live.errors += 1;
            inner.total_errors += 1;
        }
    }

    /// Close the current window and take a sample. Returns `None` for the
    /// very first call, which has no CPU delta to measure yet.
    pub fn sample(&self, in_flight: u64) -> Option<Sample> {
        let stat = process_stat();
        let now = Instant::now();
        let mut inner = self.lock();

        let cpu_pct = match (stat.cpu_seconds, inner.last_cpu) {
            (Some(total), Some((at, previous))) => {
                let elapsed = now.duration_since(at).as_secs_f64();
                let used = (total - previous).max(0.0);
                Some(if elapsed > 0.0 {
                    used / elapsed * 100.0
                } else {
                    0.0
                })
            }
            _ => None,
        };
        inner.last_cpu = stat.cpu_seconds.map(|total| (now, total));
        let Some(cpu_pct) = cpu_pct else {
            // The baseline tick: no window to close yet, nothing to write.
            return None;
        };

        let mut live = std::mem::take(&mut inner.live);
        live.secs = inner.live_started.elapsed().as_secs_f64();
        inner.live_started = now;

        let sample = Sample {
            t: Utc::now().timestamp_millis(),
            secs: live.secs,
            cpu_pct,
            rss_bytes: stat.rss_bytes.unwrap_or(0),
            threads: stat.threads.unwrap_or(0),
            in_flight,
            requests: live.requests,
            errors: live.errors,
            latency_p50_ms: live.latency.percentile(50),
            latency_p95_ms: live.latency.percentile(95),
            ttft_p50_ms: live.ttft.percentile(50),
        };

        inner.windows.push_back(live);
        while inner.windows.len() > SUMMARY_SAMPLES {
            inner.windows.pop_front();
        }
        inner.ring.push_back(sample.clone());
        while inner.ring.len() > RING_SAMPLES {
            inner.ring.pop_front();
        }
        let per_minute = {
            let mut merged = Window::default();
            for window in inner.windows.iter().rev().take(6) {
                merged.merge(window);
            }
            if merged.secs > 0.0 {
                merged.requests as f64 / merged.secs * 60.0
            } else {
                0.0
            }
        };
        drop(inner);

        if let Some(store) = &self.sqlite {
            store.record_sys(&sample);
        }
        crate::metrics::set_process(sample.cpu_pct, sample.rss_bytes, sample.threads);
        crate::metrics::set_requests_per_minute(per_minute);
        Some(sample)
    }

    /// The live summary the console's cards read.
    pub fn snapshot(&self, in_flight: u64) -> Snapshot {
        let inner = self.lock();
        let last = inner.ring.back().cloned();
        let live_secs = inner.live_started.elapsed().as_secs_f64();
        let windows = WINDOWS
            .iter()
            .map(|(seconds, label)| summarize(&inner, &inner.live, live_secs, *seconds, label))
            .collect();
        Snapshot {
            now: Utc::now().timestamp_millis(),
            started: self.started.to_rfc3339(),
            uptime_secs: self.started_instant.elapsed().as_secs(),
            interval_secs: SAMPLE_SECS,
            sampling: self.sampling.load(Ordering::Relaxed),
            cores: inner.cores,
            cpu_pct: last.as_ref().map(|s| s.cpu_pct).unwrap_or(0.0),
            rss_bytes: last.as_ref().map(|s| s.rss_bytes).unwrap_or(0),
            threads: last.as_ref().map(|s| s.threads).unwrap_or(0),
            in_flight,
            total_requests: inner.total_requests,
            total_errors: inner.total_errors,
            windows,
        }
    }

    /// The live ring, oldest first, for the chart when SQLite is off.
    pub fn series(&self, since_ms: i64) -> Vec<Sample> {
        let inner = self.lock();
        inner
            .ring
            .iter()
            .filter(|s| s.t >= since_ms)
            .cloned()
            .collect()
    }

    /// Whether a sampler tick loop is running in this process.
    pub fn is_sampling(&self) -> bool {
        self.sampling.load(Ordering::Relaxed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A monitor must never take the gateway down: recover the state.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Aggregate the windows that fit in `seconds`, plus the live one.
fn summarize(
    inner: &Inner,
    live: &Window,
    live_secs: f64,
    seconds: u64,
    label: &'static str,
) -> Summary {
    let mut merged = Window::default();
    for window in inner.windows.iter().rev() {
        if merged.secs >= seconds as f64 {
            break;
        }
        merged.merge(window);
    }
    // The still-open window's length is not in `live.secs` yet (the sampler
    // stamps it only when it closes), so add it here.
    merged.secs += live_secs;
    merged.requests += live.requests;
    merged.errors += live.errors;
    merged.latency.merge(&live.latency);
    merged.ttft.merge(&live.ttft);
    let rps = if merged.secs > 0.0 {
        merged.requests as f64 / merged.secs
    } else {
        0.0
    };
    Summary {
        label,
        seconds,
        requests: merged.requests,
        errors: merged.errors,
        rps,
        latency_p50_ms: merged.latency.percentile(50),
        latency_p95_ms: merged.latency.percentile(95),
        latency_avg_ms: merged.latency.avg(),
        ttft_p50_ms: merged.ttft.percentile(50),
    }
}

/// Merge samples into fixed-width buckets so a day of ten-second samples can
/// be drawn as a couple of hundred points.
pub fn bucketize(samples: &[Sample], bucket_ms: i64) -> Vec<Point> {
    if samples.is_empty() || bucket_ms <= 0 {
        return Vec::new();
    }
    let mut out: Vec<Point> = Vec::new();
    let mut bucket: Option<Bucket> = None;
    for sample in samples {
        let key = sample.t / bucket_ms * bucket_ms;
        match &mut bucket {
            Some(open) if open.t == key => open.add(sample),
            _ => {
                if let Some(done) = bucket.take() {
                    out.push(done.finish());
                }
                let mut fresh = Bucket::new(key);
                fresh.add(sample);
                bucket = Some(fresh);
            }
        }
    }
    if let Some(done) = bucket {
        out.push(done.finish());
    }
    out
}

struct Bucket {
    t: i64,
    secs: f64,
    cpu_weighted: f64,
    rss_bytes: u64,
    threads: u32,
    in_flight: u64,
    requests: u64,
    errors: u64,
    p50_sum: u64,
    p50_count: u64,
    p95_max: Option<u64>,
    ttft_sum: u64,
    ttft_count: u64,
}

impl Bucket {
    fn new(t: i64) -> Self {
        Bucket {
            t,
            secs: 0.0,
            cpu_weighted: 0.0,
            rss_bytes: 0,
            threads: 0,
            in_flight: 0,
            requests: 0,
            errors: 0,
            p50_sum: 0,
            p50_count: 0,
            p95_max: None,
            ttft_sum: 0,
            ttft_count: 0,
        }
    }

    fn add(&mut self, sample: &Sample) {
        self.secs += sample.secs;
        self.cpu_weighted += sample.cpu_pct * sample.secs;
        self.rss_bytes = self.rss_bytes.max(sample.rss_bytes);
        self.threads = self.threads.max(sample.threads);
        self.in_flight = self.in_flight.max(sample.in_flight);
        self.requests += sample.requests;
        self.errors += sample.errors;
        if let Some(p50) = sample.latency_p50_ms {
            self.p50_sum += p50;
            self.p50_count += 1;
        }
        if let Some(p95) = sample.latency_p95_ms {
            self.p95_max = Some(self.p95_max.map_or(p95, |m| m.max(p95)));
        }
        if let Some(ttft) = sample.ttft_p50_ms {
            self.ttft_sum += ttft;
            self.ttft_count += 1;
        }
    }

    fn finish(self) -> Point {
        Point {
            t: self.t,
            cpu_pct: if self.secs > 0.0 {
                self.cpu_weighted / self.secs
            } else {
                0.0
            },
            rss_bytes: self.rss_bytes,
            threads: self.threads,
            in_flight: self.in_flight,
            requests: self.requests,
            errors: self.errors,
            rps: if self.secs > 0.0 {
                self.requests as f64 / self.secs
            } else {
                0.0
            },
            latency_p50_ms: (self.p50_count > 0).then(|| self.p50_sum / self.p50_count),
            // The worst window in the bucket: spikes are the point of a p95.
            latency_p95_ms: self.p95_max,
            ttft_p50_ms: (self.ttft_count > 0).then(|| self.ttft_sum / self.ttft_count),
        }
    }
}

// ------------------------------------------------------------------ sampler --

/// Start the tick loop once per process. Safe to call from more than one
/// place; only the first call schedules anything.
pub fn spawn_sampler(state: AppState) {
    if state.sys.spawned.swap(true, Ordering::SeqCst) {
        return;
    }
    state.sys.sampling.store(true, Ordering::Relaxed);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(SAMPLE_SECS));
        // The first tick is immediate: it only records the CPU baseline.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let in_flight = state.in_flight.load(Ordering::Relaxed);
            if let Err(err) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                state.sys.sample(in_flight);
            })) {
                tracing::warn!("the process sampler failed: {err:?}");
            }
        }
    });
}

// --------------------------------------------------------- process readings --

struct ProcessStat {
    /// Cumulative CPU seconds (user + system) since process start.
    cpu_seconds: Option<f64>,
    rss_bytes: Option<u64>,
    threads: Option<u32>,
}

#[cfg(unix)]
fn cpu_seconds() -> Option<f64> {
    // `getrusage` is the one portable reading: Linux and macOS both answer,
    // and it needs no /proc and no libproc flavor constant.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    if rc != 0 {
        return None;
    }
    let user = usage.ru_utime.tv_sec as f64 + usage.ru_utime.tv_usec as f64 / 1_000_000.0;
    let system = usage.ru_stime.tv_sec as f64 + usage.ru_stime.tv_usec as f64 / 1_000_000.0;
    Some(user + system)
}

#[cfg(not(unix))]
fn cpu_seconds() -> Option<f64> {
    None
}

#[cfg(target_os = "linux")]
fn process_stat() -> ProcessStat {
    ProcessStat {
        cpu_seconds: cpu_seconds(),
        rss_bytes: linux_rss_bytes(),
        threads: linux_threads(),
    }
}

#[cfg(target_os = "linux")]
fn linux_rss_bytes() -> Option<u64> {
    // /proc/self/statm: size resident shared text lib data dt (in pages).
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page = if page > 0 { page as u64 } else { 4096 };
    Some(pages * page)
}

#[cfg(target_os = "linux")]
fn linux_threads() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:")?.trim().parse::<u32>().ok())
}

#[cfg(target_os = "macos")]
fn process_stat() -> ProcessStat {
    use std::mem::size_of;
    /// `PROC_PIDTASKINFO` from `<libproc.h>`; libc exposes the struct, not
    /// the constant.
    const PROC_PIDTASKINFO: libc::c_int = 4;
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
    let wrote = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    let (rss_bytes, threads) = if wrote == size {
        (
            Some(info.pti_resident_size),
            Some(info.pti_threadnum.max(0) as u32),
        )
    } else {
        (None, None)
    };
    ProcessStat {
        cpu_seconds: cpu_seconds(),
        rss_bytes,
        threads,
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn process_stat() -> ProcessStat {
    ProcessStat {
        cpu_seconds: cpu_seconds(),
        rss_bytes: None,
        threads: None,
    }
}

#[cfg(not(unix))]
fn process_stat() -> ProcessStat {
    ProcessStat {
        cpu_seconds: None,
        rss_bytes: None,
        threads: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: i64, secs: f64, requests: u64) -> Sample {
        Sample {
            t,
            secs,
            cpu_pct: 50.0,
            rss_bytes: 100 * 1024 * 1024,
            threads: 10,
            in_flight: 2,
            requests,
            errors: 0,
            latency_p50_ms: Some(40),
            latency_p95_ms: Some(400),
            ttft_p50_ms: Some(30),
        }
    }

    #[test]
    fn percentiles_land_in_the_right_doubling_bucket() {
        let mut hist = Hist::default();
        for value in 100..=199u64 {
            hist.add(value);
        }
        // 150 sits in [128, 256), so both percentiles read 128.
        assert_eq!(hist.percentile(50), Some(128));
        assert_eq!(hist.percentile(95), Some(128));
        assert_eq!(hist.avg(), Some(149));
    }

    #[test]
    fn an_empty_histogram_has_no_percentile() {
        let hist = Hist::default();
        assert_eq!(hist.percentile(50), None);
        assert_eq!(hist.avg(), None);
    }

    #[test]
    fn merging_histograms_keeps_the_counts() {
        let mut a = Hist::default();
        a.add(10);
        let mut b = Hist::default();
        b.add(10_000);
        a.merge(&b);
        assert_eq!(a.count, 2);
        assert_eq!(a.percentile(50), Some(8));
    }

    #[test]
    fn bucketize_merges_six_ten_second_samples_into_a_minute() {
        let samples: Vec<Sample> = (0..6).map(|i| sample(i * 10_000, 10.0, 100)).collect();
        let points = bucketize(&samples, 60_000);
        assert_eq!(points.len(), 1);
        let point = &points[0];
        assert_eq!(point.requests, 600);
        assert_eq!(point.rps, 10.0);
        assert_eq!(point.latency_p50_ms, Some(40));
        assert_eq!(point.latency_p95_ms, Some(400));
        assert_eq!(point.cpu_pct, 50.0);
        assert_eq!(point.threads, 10);
    }

    #[test]
    fn bucketize_keeps_spikes_as_the_bucket_p95() {
        let mut fast = sample(0, 10.0, 10);
        fast.latency_p95_ms = Some(100);
        let mut slow = sample(10_000, 10.0, 10);
        slow.latency_p95_ms = Some(9_000);
        let points = bucketize(&[fast, slow], 60_000);
        assert_eq!(points[0].latency_p95_ms, Some(9_000));
        assert_eq!(points[0].latency_p50_ms, Some(40));
    }

    #[test]
    fn an_empty_series_buckets_to_nothing() {
        assert!(bucketize(&[], 60_000).is_empty());
    }

    #[tokio::test]
    async fn the_first_sample_is_only_the_cpu_baseline() {
        let monitor = Monitor::new(None);
        assert!(monitor.sample(0).is_none());
        let second = monitor.sample(0).expect("a sample once a baseline exists");
        assert!(second.cpu_pct >= 0.0);
        assert_eq!(monitor.series(0).len(), 1);
    }

    #[tokio::test]
    async fn recorded_requests_reach_the_window_and_the_snapshot() {
        let monitor = Monitor::new(None);
        let mut rec = UsageRecord::new("glm-4.7", "openai_chat");
        rec.latency_ms = 120;
        rec.ttft_ms = Some(60);
        monitor.record(&rec);
        let mut failed = UsageRecord::new("glm-4.7", "openai_chat");
        failed.ok = false;
        failed.status = 502;
        monitor.record(&failed);

        let snapshot = monitor.snapshot(3);
        assert_eq!(snapshot.in_flight, 3);
        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.total_errors, 1);
        let minute = &snapshot.windows[0];
        assert_eq!(minute.requests, 2);
        assert_eq!(minute.errors, 1);
        // The 502 is counted, but its (unset) latency is not in the average.
        assert_eq!(minute.ttft_p50_ms, Some(32));
        assert!(minute.latency_p95_ms.is_some());
    }
}
