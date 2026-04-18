//! Background snapshot manager with deduplication and rate limiting.
//!
//! Owns a dedicated I/O thread that receives [`SnapshotRequest`]s
//! via a bounded channel, deduplicates them by content hash, enforces
//! a minimum interval between writes, and delegates the actual file
//! I/O to [`snapshot`](crate::snapshot).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tracing::{info, warn};

use crate::analyze::RuleEvaluation;
use crate::hash::{blake3_u64, blake3_u64_pair};
use crate::snapshot::{
    self, ensure_dir, now_iso8601, write_metadata_atomic, write_rle_bits_atomic, SnapshotMetadata,
};
use crate::{EdgeMode, Grid};

const DEFAULT_QUEUE_CAPACITY: usize = 64;
const MIN_QUEUE_CAPACITY: usize = 1;
const DROP_LOG_INTERVAL: Duration = Duration::from_secs(2);

/// Generous I/O-thread stack — large grid bitsets and serde buffers can
/// push past the default 2 MiB on debug builds.
const IO_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Filename prefix used for every background-manager snapshot.
const SNAPSHOT_FILENAME_PREFIX: &str = "sim";

// ── Public types ────────────────────────────────────────────────────

/// The kind of event that triggered a snapshot.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SnapshotEventKind {
    FixedPoint,
    Cycle,
    NewBestRule,
    Manual,
}

/// All data needed to write one snapshot to disk.
#[derive(Clone, Debug)]
pub struct SnapshotRequest {
    pub event: SnapshotEventKind,
    pub timestamp: SystemTime,
    pub gen: u64,
    pub rule: String,
    pub width: u16,
    pub height: u16,
    pub wrap: EdgeMode,
    pub seed_hash: u64,
    pub grid_hash: [u64; 2],
    pub grid_bits: Vec<u64>,
    pub period: Option<u64>,
    pub transient: Option<u64>,
    pub score: Option<f32>,
    pub meta: SnapshotMetadata,
}

/// Cumulative statistics reported by the snapshot manager.
#[derive(Clone, Debug)]
pub struct SnapshotStats {
    pub written: u64,
    pub dropped: u64,
    pub queue_len: usize,
    pub last_path: Option<PathBuf>,
}

/// Configuration for constructing a [`SnapshotManager`].
#[derive(Clone, Debug)]
pub struct SnapshotManagerConfig {
    pub dir: PathBuf,
    pub max_files: usize,
    pub min_interval_ms: u64,
    pub queue_capacity: usize,
}

impl SnapshotManagerConfig {
    pub fn new(dir: PathBuf, max_files: usize, min_interval_ms: u64) -> Self {
        Self {
            dir,
            max_files,
            min_interval_ms,
            queue_capacity: snapshot_queue_capacity(),
        }
    }
}

// ── Snapshot manager ────────────────────────────────────────────────

/// Manages asynchronous snapshot writing on a background I/O thread.
pub struct SnapshotManager {
    inner: Arc<SnapshotManagerInner>,
    handle: Option<JoinHandle<()>>,
    #[cfg(test)]
    #[allow(dead_code)]
    rx_guard: Option<Receiver<IoCommand>>,
}

struct SnapshotManagerInner {
    tx: Sender<IoCommand>,
    last_enqueued: Mutex<LastSnapshotKey>,
    dropped: AtomicU64,
    written: AtomicU64,
    last_path: Mutex<Option<PathBuf>>,
    last_drop_log: Mutex<Instant>,
    min_interval: Duration,
    dir: PathBuf,
    max_files: usize,
}

impl SnapshotManager {
    pub fn new(config: SnapshotManagerConfig) -> Self {
        let (inner, rx) = build_inner(&config);
        let handle = spawn_worker(rx, Arc::clone(&inner));
        Self {
            inner,
            handle,
            #[cfg(test)]
            rx_guard: None,
        }
    }

    #[cfg(test)]
    fn new_for_tests(config: SnapshotManagerConfig) -> Self {
        let (inner, rx) = build_inner(&config);
        Self {
            inner,
            handle: None,
            rx_guard: Some(rx),
        }
    }

    /// Try to enqueue a snapshot request. Returns `false` if the
    /// request was dropped (dedup, cooldown, or full queue).
    pub fn enqueue(&self, req: SnapshotRequest) -> bool {
        let key = SnapshotKey::from_request(&req);
        let now = Instant::now();
        if !self.admit(&key, req.event, now) {
            return false;
        }
        self.dispatch_snapshot(req, key, now)
    }

    /// Enqueue a rule-log entry for append to the JSON-lines log.
    pub fn record_rule(&self, entry: RuleLogEntry) -> bool {
        self.try_send(IoCommand::RecordRule(entry))
    }

    /// Read current cumulative statistics (non-blocking).
    pub fn stats(&self) -> SnapshotStats {
        SnapshotStats {
            written: self.inner.written.load(Ordering::Relaxed),
            dropped: self.inner.dropped.load(Ordering::Relaxed),
            queue_len: self.inner.tx.len(),
            last_path: self.inner.last_path.lock().unwrap().clone(),
        }
    }

    /// Gracefully shut down the background I/O thread.
    pub fn shutdown(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = self.inner.tx.send(IoCommand::Shutdown);
            let _ = handle.join();
        }
    }

    /// Check dedup + cooldown rules; drop the request if rejected.
    fn admit(&self, key: &SnapshotKey, event: SnapshotEventKind, now: Instant) -> bool {
        let guard = self.inner.last_enqueued.lock().unwrap();
        if guard.allows(key, event, now, self.inner.min_interval) {
            return true;
        }
        self.inner.dropped.fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Send an admitted request and update the dedup key on success.
    fn dispatch_snapshot(&self, req: SnapshotRequest, key: SnapshotKey, now: Instant) -> bool {
        match self.inner.tx.try_send(IoCommand::Snapshot(Box::new(req))) {
            Ok(()) => {
                let mut guard = self.inner.last_enqueued.lock().unwrap();
                guard.key = Some(key);
                guard.last_at = now;
                true
            }
            Err(err) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
                if matches!(err, TrySendError::Full(_)) {
                    self.maybe_log_drop();
                }
                false
            }
        }
    }

    fn try_send(&self, cmd: IoCommand) -> bool {
        match self.inner.tx.try_send(cmd) {
            Ok(()) => true,
            Err(err) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
                if matches!(err, TrySendError::Full(_)) {
                    self.maybe_log_drop();
                }
                false
            }
        }
    }

    fn maybe_log_drop(&self) {
        let now = Instant::now();
        let mut last = self.inner.last_drop_log.lock().unwrap();
        if now.duration_since(*last) >= DROP_LOG_INTERVAL {
            warn!("Snapshot queue full; dropping snapshot");
            *last = now;
        }
    }
}

impl Drop for SnapshotManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn build_inner(config: &SnapshotManagerConfig) -> (Arc<SnapshotManagerInner>, Receiver<IoCommand>) {
    let (tx, rx) = bounded(config.queue_capacity.max(MIN_QUEUE_CAPACITY));
    let min_interval = Duration::from_millis(config.min_interval_ms.max(1));
    let now = Instant::now();
    let inner = Arc::new(SnapshotManagerInner {
        tx,
        last_enqueued: Mutex::new(LastSnapshotKey {
            key: None,
            last_at: saturating_past(now, min_interval),
        }),
        dropped: AtomicU64::new(0),
        written: AtomicU64::new(0),
        last_path: Mutex::new(None),
        last_drop_log: Mutex::new(saturating_past(now, DROP_LOG_INTERVAL)),
        min_interval,
        dir: config.dir.clone(),
        max_files: config.max_files,
    });
    (inner, rx)
}

/// Shift `now` back by `back`, saturating at `now` when the subtraction
/// would underflow — used to seed "initial" timers so the first request
/// is not artificially throttled.
fn saturating_past(now: Instant, back: Duration) -> Instant {
    now.checked_sub(back).unwrap_or(now)
}

// ── Deduplication key ───────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
struct SnapshotKey {
    event_kind: SnapshotEventKind,
    rule_hash: u64,
    seed_hash: u64,
    grid_hash: [u64; 2],
    period: Option<u64>,
}

impl SnapshotKey {
    fn from_request(req: &SnapshotRequest) -> Self {
        Self {
            event_kind: req.event,
            rule_hash: rule_hash(&req.rule),
            seed_hash: req.seed_hash,
            grid_hash: req.grid_hash,
            period: req.period,
        }
    }
}

struct LastSnapshotKey {
    key: Option<SnapshotKey>,
    last_at: Instant,
}

impl LastSnapshotKey {
    /// Manual events bypass the cooldown but still honor dedup.
    fn allows(
        &self,
        key: &SnapshotKey,
        event_kind: SnapshotEventKind,
        now: Instant,
        min_interval: Duration,
    ) -> bool {
        if let Some(last) = &self.key {
            if last == key {
                return false;
            }
        }
        if matches!(event_kind, SnapshotEventKind::Manual) {
            return true;
        }
        now.duration_since(self.last_at) >= min_interval
    }
}

// ── I/O worker ──────────────────────────────────────────────────────

enum IoCommand {
    Snapshot(Box<SnapshotRequest>),
    RecordRule(RuleLogEntry),
    Shutdown,
}

fn spawn_worker(
    rx: Receiver<IoCommand>,
    inner: Arc<SnapshotManagerInner>,
) -> Option<JoinHandle<()>> {
    if let Err(err) = fs::create_dir_all(&inner.dir) {
        warn!("Snapshot dir init failed: {}", err);
    }
    let builder = thread::Builder::new()
        .name("nit-gol-io".into())
        .stack_size(IO_THREAD_STACK_BYTES);
    match builder.spawn(move || worker_loop(rx, inner)) {
        Ok(handle) => Some(handle),
        Err(err) => {
            warn!("Failed to spawn snapshot worker: {}", err);
            None
        }
    }
}

fn worker_loop(rx: Receiver<IoCommand>, inner: Arc<SnapshotManagerInner>) {
    while let Ok(cmd) = rx.recv() {
        let (result, label): (io::Result<()>, &str) = match cmd {
            IoCommand::Snapshot(req) => (handle_snapshot(*req, &inner), "Snapshot"),
            IoCommand::RecordRule(entry) => (append_rule_entry(entry), "Snapshot rule log"),
            IoCommand::Shutdown => break,
        };
        if let Err(err) = result {
            warn!("{} failed: {}", label, err);
        }
    }
}

fn handle_snapshot(req: SnapshotRequest, inner: &SnapshotManagerInner) -> io::Result<()> {
    ensure_dir(&inner.dir)?;
    let rle_path = write_snapshot_files(&inner.dir, &req)?;
    record_write_success(inner, rle_path);
    let _ = snapshot::prune_oldest(&inner.dir, inner.max_files);
    Ok(())
}

fn write_snapshot_files(dir: &Path, req: &SnapshotRequest) -> io::Result<PathBuf> {
    // The legacy stem embeds only the low 32 bits of the grid hash —
    // widen before calling the shared formatter so `:08x` produces the
    // same 8-char output that existing on-disk snapshots use.
    let hash_low32 = u64::from(req.grid_hash[0] as u32);
    let stem = snapshot::format_name_stem(
        Some(SNAPSHOT_FILENAME_PREFIX),
        &req.meta.timestamp,
        &req.rule,
        req.gen,
        hash_low32,
    );
    let rle_path = dir.join(format!("{stem}.rle"));
    let json_path = dir.join(format!("{stem}.json"));
    write_rle_bits_atomic(&rle_path, req.width, req.height, &req.rule, &req.grid_bits)?;
    write_metadata_atomic(&json_path, &req.meta)?;
    Ok(rle_path)
}

fn record_write_success(inner: &SnapshotManagerInner, rle_path: PathBuf) {
    inner.written.fetch_add(1, Ordering::Relaxed);
    info!("Snapshot saved: {}", rle_path.display());
    *inner.last_path.lock().unwrap() = Some(rle_path);
}

// ── Hashing helpers ─────────────────────────────────────────────────

fn rule_hash(rule: &str) -> u64 {
    blake3_u64(&blake3::hash(rule.as_bytes()))
}

/// Two-word blake3 fingerprint of a grid's dimensions and cells.
///
/// The domain tag `nit-gol-snapshot-v1` is part of the on-disk
/// fingerprint contract — changing it invalidates existing snapshots.
pub fn grid_fingerprint(grid: &Grid) -> [u64; 2] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"nit-gol-snapshot-v1");
    hasher.update(&grid.width().to_le_bytes());
    hasher.update(&grid.height().to_le_bytes());
    hasher.update(grid.cells());
    blake3_u64_pair(&hasher.finalize())
}

/// Pack grid cells into a `u64` bitset for compact snapshot storage.
///
/// Bit `i` of word `i/64` corresponds to cell `i` in row-major order.
pub fn pack_grid_bits(grid: &Grid) -> Vec<u64> {
    let total = grid.width().saturating_mul(grid.height());
    let mut bits = vec![0u64; total.div_ceil(64)];
    for (idx, &cell) in grid.cells().iter().enumerate() {
        if cell != 0 {
            bits[idx / 64] |= 1u64 << (idx % 64);
        }
    }
    bits
}

/// Read the snapshot queue capacity from `NIT_SNAPSHOT_QUEUE` or use 64.
pub fn snapshot_queue_capacity() -> usize {
    std::env::var("NIT_SNAPSHOT_QUEUE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_QUEUE_CAPACITY)
        .max(MIN_QUEUE_CAPACITY)
}

// ── Rule log ────────────────────────────────────────────────────────

/// A scored-rule discovery record appended to the JSON-lines log.
#[derive(Clone, Debug, serde::Serialize)]
pub struct RuleLogEntry {
    rule: String,
    score: f32,
    discovered_at: String,
    seed_hash: u64,
    notes: String,
    /// Destination log file; routed by the worker, not serialized.
    #[serde(skip)]
    path: PathBuf,
}

impl RuleLogEntry {
    /// Create an entry from a completed rule evaluation.
    pub fn from_eval(eval: &RuleEvaluation, seed_hash: u64, path: &Path) -> Self {
        Self {
            rule: eval.rule.to_string(),
            score: eval.score,
            discovered_at: now_iso8601(),
            seed_hash,
            notes: format!(
                "period={:?} transient={} alive_end={}",
                eval.period, eval.transient, eval.alive_end
            ),
            path: path.to_path_buf(),
        }
    }
}

fn append_rule_entry(entry: RuleLogEntry) -> io::Result<()> {
    if let Some(parent) = entry.path.parent().filter(|p| !p.exists()) {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&entry.path)?;
    serde_json::to_writer(&mut file, &entry).map_err(io::Error::other)?;
    use io::Write;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
#[path = "test_modules/snapshot_manager.rs"]
mod tests;
