//! Public types and constants for the snapshot manager.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::snapshot::SnapshotMetadata;
use crate::EdgeMode;

pub(super) const DEFAULT_QUEUE_CAPACITY: usize = 64;
pub(super) const MIN_QUEUE_CAPACITY: usize = 1;

/// Filename prefix used for every background-manager snapshot.
pub(super) const SNAPSHOT_FILENAME_PREFIX: &str = "sim";

/// Generous I/O-thread stack — large grid bitsets and serde buffers can
/// push past the default 2 MiB on debug builds.
pub(super) const IO_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

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

/// Configuration for constructing a [`SnapshotManager`](super::SnapshotManager).
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

/// Read the snapshot queue capacity from `NIT_SNAPSHOT_QUEUE` or use 64.
pub fn snapshot_queue_capacity() -> usize {
    std::env::var("NIT_SNAPSHOT_QUEUE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_QUEUE_CAPACITY)
        .max(MIN_QUEUE_CAPACITY)
}
