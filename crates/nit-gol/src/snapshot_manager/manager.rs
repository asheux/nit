//! The public [`SnapshotManager`] façade and the shared inner state
//! consumed by the worker thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use tracing::warn;

use super::dedup::{LastSnapshotKey, SnapshotKey};
use super::rule_log::RuleLogEntry;
use super::types::{
    SnapshotEventKind, SnapshotManagerConfig, SnapshotRequest, SnapshotStats, MIN_QUEUE_CAPACITY,
};
use super::worker::{spawn_worker, IoCommand};

const DROP_LOG_INTERVAL: Duration = Duration::from_secs(2);

/// Manages asynchronous snapshot writing on a background I/O thread.
pub struct SnapshotManager {
    inner: Arc<SnapshotManagerInner>,
    handle: Option<JoinHandle<()>>,
    #[cfg(test)]
    #[allow(dead_code)]
    rx_guard: Option<Receiver<IoCommand>>,
}

pub(super) struct SnapshotManagerInner {
    pub(super) tx: Sender<IoCommand>,
    pub(super) last_enqueued: Mutex<LastSnapshotKey>,
    pub(super) dropped: AtomicU64,
    pub(super) written: AtomicU64,
    pub(super) last_path: Mutex<Option<PathBuf>>,
    pub(super) last_drop_log: Mutex<Instant>,
    pub(super) min_interval: Duration,
    pub(super) dir: PathBuf,
    pub(super) max_files: usize,
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
    pub(super) fn new_for_tests(config: SnapshotManagerConfig) -> Self {
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
