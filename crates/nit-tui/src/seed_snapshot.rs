use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use serde::Serialize;
use tracing::warn;

use nit_gol::snapshot::{now_iso8601, write_rle_bits_atomic};
use nit_gol::Rule;

const DEFAULT_QUEUE_CAPACITY: usize = 64;
const MIN_QUEUE_CAPACITY: usize = 1;
const DROP_LOG_INTERVAL: Duration = Duration::from_secs(2);
const IO_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct SeedSnapshotMetadata {
    pub timestamp: String,
    pub workspace_root: Option<String>,
    pub file_path: Option<String>,
    pub source: String,
    pub revision: u64,
    pub encoder_id: String,
    pub encoder_params: String,
    pub params_fingerprint: u64,
    pub seed_hash: u64,
    pub input_hash: u64,
    pub density: f32,
    pub symmetry: String,
    pub components: usize,
    pub width: usize,
    pub height: usize,
    pub view_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genome_preview: Option<SeedGenomePreview>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SeedGenomePreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifehash16_bits: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hilbert_bits_prefix: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SeedSnapshotRequest {
    pub timestamp: SystemTime,
    pub name_base: String,
    pub width: u16,
    pub height: u16,
    pub grid_bits: Vec<u64>,
    pub meta: SeedSnapshotMetadata,
}

#[derive(Clone, Debug)]
pub struct SeedSnapshotStats {
    pub written: u64,
    pub dropped: u64,
    pub queue_len: usize,
    pub last_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct SeedSnapshotManagerConfig {
    pub dir: PathBuf,
    pub max_files: usize,
    pub min_interval_ms: u64,
    pub queue_capacity: usize,
}

impl SeedSnapshotManagerConfig {
    pub fn new(dir: PathBuf, max_files: usize, min_interval_ms: u64) -> Self {
        Self {
            dir,
            max_files,
            min_interval_ms,
            queue_capacity: snapshot_queue_capacity(),
        }
    }
}

pub struct SeedSnapshotManager {
    inner: Arc<SeedSnapshotInner>,
    handle: Option<JoinHandle<()>>,
    #[cfg(test)]
    #[allow(dead_code)]
    rx_guard: Option<Receiver<SeedIoCommand>>,
}

struct SeedSnapshotInner {
    tx: Sender<SeedIoCommand>,
    last_key: Mutex<LastSeedSnapshotKey>,
    dropped: AtomicU64,
    written: AtomicU64,
    last_path: Mutex<Option<PathBuf>>,
    last_drop_log: Mutex<Instant>,
    min_interval: Duration,
    dir: PathBuf,
    max_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SeedSnapshotKey {
    encoder_id: String,
    seed_hash: u64,
    params_fingerprint: u64,
}

struct LastSeedSnapshotKey {
    key: Option<SeedSnapshotKey>,
    last_at: Instant,
}

impl LastSeedSnapshotKey {
    fn allows(&self, key: &SeedSnapshotKey, now: Instant, min_interval: Duration) -> bool {
        if let Some(prev) = &self.key {
            if prev == key {
                return false;
            }
        }
        if now.duration_since(self.last_at) < min_interval {
            return false;
        }
        true
    }
}

enum SeedIoCommand {
    Snapshot(SeedSnapshotRequest),
    Shutdown,
}

impl SeedSnapshotManager {
    pub fn new(config: SeedSnapshotManagerConfig) -> Self {
        let (tx, rx) = bounded(config.queue_capacity.max(MIN_QUEUE_CAPACITY));
        let min_interval = Duration::from_millis(config.min_interval_ms.max(1));
        let now = Instant::now();
        let inner = Arc::new(SeedSnapshotInner {
            tx,
            last_key: Mutex::new(LastSeedSnapshotKey {
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

    pub fn enqueue(&self, req: SeedSnapshotRequest) -> bool {
        let key = SeedSnapshotKey {
            encoder_id: req.meta.encoder_id.clone(),
            seed_hash: req.meta.seed_hash,
            params_fingerprint: req.meta.params_fingerprint,
        };
        let now = Instant::now();
        {
            let mut last = self.inner.last_key.lock().unwrap();
            if !last.allows(&key, now, self.inner.min_interval) {
                return false;
            }
            last.key = Some(key);
            last.last_at = now;
        }
        match self.inner.tx.try_send(SeedIoCommand::Snapshot(req)) {
            Ok(_) => true,
            Err(TrySendError::Full(_)) => {
                self.note_drop();
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn stats(&self) -> SeedSnapshotStats {
        SeedSnapshotStats {
            written: self.inner.written.load(Ordering::Relaxed),
            dropped: self.inner.dropped.load(Ordering::Relaxed),
            queue_len: self.inner.tx.len(),
            last_path: self.inner.last_path.lock().unwrap().clone(),
        }
    }

    pub fn shutdown(&mut self) {
        let _ = self.inner.tx.send(SeedIoCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn note_drop(&self) {
        self.inner.dropped.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        let mut last_log = self.inner.last_drop_log.lock().unwrap();
        if now.duration_since(*last_log) >= DROP_LOG_INTERVAL {
            *last_log = now;
            warn!("Seed snapshot queue full; dropping");
        }
    }
}

impl Drop for SeedSnapshotManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spawn_worker(
    rx: Receiver<SeedIoCommand>,
    inner: Arc<SeedSnapshotInner>,
) -> Option<JoinHandle<()>> {
    thread::Builder::new()
        .name("nit-seed-io".into())
        .stack_size(IO_THREAD_STACK_BYTES)
        .spawn(move || seed_worker_loop(rx, inner))
        .ok()
}

fn seed_worker_loop(rx: Receiver<SeedIoCommand>, inner: Arc<SeedSnapshotInner>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            SeedIoCommand::Snapshot(req) => {
                if let Err(err) = handle_seed_snapshot(req, &inner) {
                    warn!("Seed snapshot failed: {}", err);
                }
            }
            SeedIoCommand::Shutdown => break,
        }
    }
}

fn handle_seed_snapshot(
    req: SeedSnapshotRequest,
    inner: &SeedSnapshotInner,
) -> std::io::Result<()> {
    ensure_dir(&inner.dir)?;
    let rle_path = inner.dir.join(format!("{}.rle", req.name_base));
    let json_path = inner.dir.join(format!("{}.json", req.name_base));
    let rule = Rule::conway().to_string();
    write_rle_bits_atomic(&rle_path, req.width, req.height, &rule, &req.grid_bits)?;
    write_seed_metadata_atomic(&json_path, &req.meta)?;
    inner.written.fetch_add(1, Ordering::Relaxed);
    *inner.last_path.lock().unwrap() = Some(rle_path);
    let _ = prune_oldest_seed(&inner.dir, inner.max_files);
    Ok(())
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

fn write_seed_metadata_atomic(path: &Path, meta: &SeedSnapshotMetadata) -> io::Result<()> {
    write_atomic(path, |writer| {
        serde_json::to_writer(writer, meta).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    })
}

fn write_atomic<F>(path: &Path, write_fn: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    let tmp_path = path.with_extension("tmp");
    let file = File::create(&tmp_path)?;
    let mut writer = BufWriter::new(file);
    write_fn(&mut writer)?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

fn prune_oldest_seed(dir: &Path, max_files: usize) -> std::io::Result<()> {
    nit_gol::snapshot::prune_oldest(dir, max_files)
}

pub fn seed_snapshot_name_base(encoder_id: &str, seed_hash: u64) -> String {
    let timestamp = now_iso8601().replace(':', "-");
    format!(
        "seed__{}__enc-{}__seedhash-{seed_hash:08x}",
        timestamp, encoder_id
    )
}

pub fn snapshot_queue_capacity() -> usize {
    let from_env = std::env::var("NIT_SNAPSHOT_QUEUE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    from_env
        .unwrap_or(DEFAULT_QUEUE_CAPACITY)
        .max(MIN_QUEUE_CAPACITY)
}

pub fn pack_grid_bits(grid: &nit_gol::Grid) -> Vec<u64> {
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
