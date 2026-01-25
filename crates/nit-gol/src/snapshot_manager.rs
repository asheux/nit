use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tracing::{info, warn};

use crate::analyze::RuleEvaluation;
use crate::snapshot::{now_iso8601, write_metadata_atomic, write_rle_bits_atomic, SnapshotMetadata};
use crate::{EdgeMode, Grid};

const DEFAULT_QUEUE_CAPACITY: usize = 64;
const MIN_QUEUE_CAPACITY: usize = 1;
const DROP_LOG_INTERVAL: Duration = Duration::from_secs(2);
const IO_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SnapshotEventKind {
    FixedPoint,
    Cycle,
    NewBestRule,
    Manual,
}

impl SnapshotEventKind {
    fn label(self) -> &'static str {
        match self {
            SnapshotEventKind::FixedPoint => "FIXED",
            SnapshotEventKind::Cycle => "CYCLE",
            SnapshotEventKind::NewBestRule => "BEST",
            SnapshotEventKind::Manual => "MANUAL",
        }
    }

    fn is_manual(self) -> bool {
        matches!(self, SnapshotEventKind::Manual)
    }
}

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

#[derive(Clone, Debug)]
pub struct SnapshotStats {
    pub written: u64,
    pub dropped: u64,
    pub queue_len: usize,
    pub last_path: Option<PathBuf>,
}

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

struct LastSnapshotKey {
    key: Option<SnapshotKey>,
    last_at: Instant,
}

impl LastSnapshotKey {
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
        if !event_kind.is_manual() {
            if now.duration_since(self.last_at) < min_interval {
                return false;
            }
        }
        true
    }
}

enum IoCommand {
    Snapshot(SnapshotRequest),
    RecordRule(RuleLogEntry),
    Shutdown,
}

impl SnapshotManager {
    pub fn new(config: SnapshotManagerConfig) -> Self {
        let (tx, rx) = bounded(config.queue_capacity.max(MIN_QUEUE_CAPACITY));
        let min_interval = Duration::from_millis(config.min_interval_ms.max(1));
        let now = Instant::now();
        let inner = Arc::new(SnapshotManagerInner {
            tx,
            last_enqueued: Mutex::new(LastSnapshotKey {
                key: None,
                last_at: now.checked_sub(min_interval).unwrap_or(now),
            }),
            dropped: AtomicU64::new(0),
            written: AtomicU64::new(0),
            last_path: Mutex::new(None),
            last_drop_log: Mutex::new(now.checked_sub(DROP_LOG_INTERVAL).unwrap_or(now)),
            min_interval,
            dir: config.dir.clone(),
            max_files: config.max_files,
        });

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
        let (tx, rx) = bounded(config.queue_capacity.max(MIN_QUEUE_CAPACITY));
        let min_interval = Duration::from_millis(config.min_interval_ms.max(1));
        let now = Instant::now();
        let inner = Arc::new(SnapshotManagerInner {
            tx,
            last_enqueued: Mutex::new(LastSnapshotKey {
                key: None,
                last_at: now.checked_sub(min_interval).unwrap_or(now),
            }),
            dropped: AtomicU64::new(0),
            written: AtomicU64::new(0),
            last_path: Mutex::new(None),
            last_drop_log: Mutex::new(now.checked_sub(DROP_LOG_INTERVAL).unwrap_or(now)),
            min_interval,
            dir: config.dir.clone(),
            max_files: config.max_files,
        });
        Self {
            inner,
            handle: None,
            rx_guard: Some(rx),
        }
    }

    pub fn enqueue(&self, req: SnapshotRequest) -> bool {
        let key = SnapshotKey::from_request(&req);
        let now = Instant::now();
        {
            let guard = self.inner.last_enqueued.lock().unwrap();
            if !guard.allows(&key, req.event, now, self.inner.min_interval) {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            }
        }

        match self.inner.tx.try_send(IoCommand::Snapshot(req)) {
            Ok(()) => {
                let mut guard = self.inner.last_enqueued.lock().unwrap();
                guard.key = Some(key);
                guard.last_at = now;
                true
            }
            Err(TrySendError::Full(_)) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
                self.maybe_log_drop();
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn record_rule(&self, entry: RuleLogEntry) -> bool {
        match self.inner.tx.try_send(IoCommand::RecordRule(entry)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
                self.maybe_log_drop();
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn stats(&self) -> SnapshotStats {
        SnapshotStats {
            written: self.inner.written.load(Ordering::Relaxed),
            dropped: self.inner.dropped.load(Ordering::Relaxed),
            queue_len: self.inner.tx.len(),
            last_path: self.inner.last_path.lock().unwrap().clone(),
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = self.inner.tx.send(IoCommand::Shutdown);
            let _ = handle.join();
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

fn spawn_worker(rx: Receiver<IoCommand>, inner: Arc<SnapshotManagerInner>) -> Option<JoinHandle<()>> {
    let dir = inner.dir.clone();
    if let Err(err) = fs::create_dir_all(&dir) {
        warn!("Snapshot dir init failed: {}", err);
    }
    let builder = thread::Builder::new()
        .name("nit-gol-io".into())
        .stack_size(IO_THREAD_STACK_BYTES);
    match builder.spawn(move || snapshot_worker_loop(rx, inner)) {
        Ok(handle) => Some(handle),
        Err(err) => {
            warn!("Failed to spawn snapshot worker: {}", err);
            None
        }
    }
}

fn snapshot_worker_loop(rx: Receiver<IoCommand>, inner: Arc<SnapshotManagerInner>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            IoCommand::Snapshot(req) => {
                if let Err(err) = handle_snapshot(req, &inner) {
                    warn!("Snapshot failed: {}", err);
                }
            }
            IoCommand::RecordRule(entry) => {
                if let Err(err) = append_rule_entry(entry) {
                    warn!("Snapshot rule log failed: {}", err);
                }
            }
            IoCommand::Shutdown => break,
        }
    }
}

fn handle_snapshot(req: SnapshotRequest, inner: &SnapshotManagerInner) -> std::io::Result<()> {
    ensure_dir(&inner.dir)?;
    let name_base = snapshot_name_base(&req);
    let rle_path = inner.dir.join(format!("{name_base}.rle"));
    let json_path = inner.dir.join(format!("{name_base}.json"));
    write_rle_bits_atomic(&rle_path, req.width, req.height, &req.rule, &req.grid_bits)?;
    write_metadata_atomic(&json_path, &req.meta)?;
    inner.written.fetch_add(1, Ordering::Relaxed);
    *inner.last_path.lock().unwrap() = Some(rle_path.clone());
    info!("Snapshot saved: {}", rle_path.display());
    let _ = prune_oldest_snapshots(&inner.dir, inner.max_files);
    Ok(())
}

fn snapshot_name_base(req: &SnapshotRequest) -> String {
    let timestamp = req.meta.timestamp.replace(':', "-");
    let rule_tag = req.rule.replace('/', "");
    let hash = req.grid_hash[0] as u32;
    format!(
        "{timestamp}__event-{}__rule-{}__gen-{gen:05}__hash-{hash:08x}",
        req.event.label(),
        rule_tag,
        gen = req.gen
    )
}

fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "snapshot dir is a symlink",
            ));
        }
        if meta.is_dir() {
            return Ok(());
        }
    }
    fs::create_dir_all(dir)
}

fn prune_oldest_snapshots(dir: &Path, max_files: usize) -> std::io::Result<()> {
    crate::snapshot::prune_oldest(dir, max_files)
}

fn rule_hash(rule: &str) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(rule.as_bytes());
    let bytes = hasher.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&bytes.as_bytes()[..8]);
    u64::from_le_bytes(out)
}

pub fn snapshot_queue_capacity() -> usize {
    let from_env = std::env::var("NIT_SNAPSHOT_QUEUE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    from_env
        .unwrap_or(DEFAULT_QUEUE_CAPACITY)
        .max(MIN_QUEUE_CAPACITY)
}

pub fn grid_fingerprint(grid: &Grid) -> [u64; 2] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"nit-gol-snapshot-v1");
    hasher.update(&grid.width().to_le_bytes());
    hasher.update(&grid.height().to_le_bytes());
    hasher.update(grid.cells());
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    let mut a = [0u8; 8];
    let mut b = [0u8; 8];
    a.copy_from_slice(&bytes[0..8]);
    b.copy_from_slice(&bytes[8..16]);
    [u64::from_le_bytes(a), u64::from_le_bytes(b)]
}

pub fn pack_grid_bits(grid: &Grid) -> Vec<u64> {
    let total = grid.width().saturating_mul(grid.height());
    let mut bits = vec![0u64; (total + 63) / 64];
    for (idx, &cell) in grid.cells().iter().enumerate() {
        if cell != 0 {
            let word = idx / 64;
            let offset = idx % 64;
            bits[word] |= 1u64 << offset;
        }
    }
    bits
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RuleLogEntry {
    rule: String,
    score: f32,
    discovered_at: String,
    seed_hash: u64,
    notes: String,
    #[serde(skip)]
    path: PathBuf,
}

impl RuleLogEntry {
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

fn append_rule_entry(entry: RuleLogEntry) -> std::io::Result<()> {
    if let Some(parent) = entry.path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&entry.path)?;
    serde_json::to_writer(&mut file, &entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    use std::io::Write;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::SnapshotMetadata;
    use crate::Rule;

    fn dummy_meta() -> SnapshotMetadata {
        SnapshotMetadata {
            timestamp: "2026-01-25T00:00:00Z".into(),
            workspace_root: None,
            file_path: None,
            seed_source: "test".into(),
            seed_hash: 1,
            rule: "B3/S23".into(),
            generation: 1,
            alive_count: 0,
            period: None,
            score: None,
            wrap_mode: "dead".into(),
            tick_ms: 1,
            attractor: None,
        }
    }

    fn dummy_req(event: SnapshotEventKind, grid_hash: [u64; 2], period: Option<u64>) -> SnapshotRequest {
        SnapshotRequest {
            event,
            timestamp: SystemTime::now(),
            gen: 1,
            rule: Rule::conway().to_string(),
            width: 2,
            height: 2,
            wrap: EdgeMode::Dead,
            seed_hash: 42,
            grid_hash,
            grid_bits: vec![0],
            period,
            transient: None,
            score: None,
            meta: dummy_meta(),
        }
    }

    #[test]
    fn snapshot_key_dedupes() {
        let req1 = dummy_req(SnapshotEventKind::Cycle, [1, 2], Some(2));
        let req2 = dummy_req(SnapshotEventKind::Cycle, [1, 2], Some(2));
        let req3 = dummy_req(SnapshotEventKind::Cycle, [1, 3], Some(2));
        assert_eq!(SnapshotKey::from_request(&req1), SnapshotKey::from_request(&req2));
        assert_ne!(SnapshotKey::from_request(&req1), SnapshotKey::from_request(&req3));
    }

    #[test]
    fn cooldown_blocks_non_manual() {
        let now = Instant::now();
        let key = SnapshotKey {
            event_kind: SnapshotEventKind::Cycle,
            rule_hash: 1,
            seed_hash: 1,
            grid_hash: [1, 2],
            period: Some(2),
        };
        let gate = LastSnapshotKey {
            key: Some(key.clone()),
            last_at: now,
        };
        let later = now + Duration::from_millis(10);
        assert!(!gate.allows(&key, SnapshotEventKind::Cycle, later, Duration::from_millis(500)));
        let other_key = SnapshotKey {
            grid_hash: [3, 4],
            ..key
        };
        assert!(gate.allows(
            &other_key,
            SnapshotEventKind::Manual,
            later,
            Duration::from_millis(500)
        ));
    }

    #[test]
    fn bounded_queue_drops_when_full() {
        let dir = std::env::temp_dir().join("nit-snapshot-test");
        let config = SnapshotManagerConfig {
            dir,
            max_files: 0,
            min_interval_ms: 0,
            queue_capacity: 1,
        };
        let manager = SnapshotManager::new_for_tests(config);
        let req1 = dummy_req(SnapshotEventKind::Manual, [1, 2], None);
        let req2 = dummy_req(SnapshotEventKind::Manual, [3, 4], None);
        assert!(manager.enqueue(req1));
        assert!(!manager.enqueue(req2));
        let stats = manager.stats();
        assert_eq!(stats.dropped, 1);
        assert_eq!(stats.queue_len, 1);
    }
}
