//! Background snapshot manager with deduplication and rate limiting.
//!
//! Owns a dedicated I/O thread that receives [`SnapshotRequest`]s
//! via a bounded channel, deduplicates them by content hash, enforces
//! a minimum interval between writes, and delegates the actual file
//! I/O to [`snapshot`](crate::snapshot).

mod bits;
mod dedup;
mod manager;
mod rule_log;
mod types;
mod worker;

pub use bits::{grid_fingerprint, pack_grid_bits};
pub use manager::SnapshotManager;
pub use rule_log::RuleLogEntry;
pub use types::{
    snapshot_queue_capacity, SnapshotEventKind, SnapshotManagerConfig, SnapshotRequest,
    SnapshotStats,
};

// Bring the full set of items that `test_modules/snapshot_manager.rs` reaches
// for via `use super::*;` into the mod scope — tests manipulate the dedup
// gate's private struct-literal fields directly.
#[cfg(test)]
#[allow(unused_imports)]
use dedup::{LastSnapshotKey, SnapshotKey};
#[cfg(test)]
#[allow(unused_imports)]
use std::time::{Duration, Instant, SystemTime};

#[cfg(test)]
#[allow(unused_imports)]
use crate::EdgeMode;

#[cfg(test)]
#[path = "../test_modules/snapshot_manager.rs"]
mod tests;
