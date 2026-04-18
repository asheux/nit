//! Background snapshot manager with deduplication and rate limiting.
//!
//! Owns a dedicated I/O thread that receives [`SnapshotRequest`]s
//! via a bounded channel, deduplicates them by content hash, enforces
//! a minimum interval between writes, and delegates the actual file
//! I/O to [`snapshot`](crate::snapshot).

mod dedup;
mod manager;
mod rule_log;
mod types;
mod worker;

pub use manager::SnapshotManager;
pub use rule_log::RuleLogEntry;
pub use types::{
    snapshot_queue_capacity, SnapshotEventKind, SnapshotManagerConfig, SnapshotRequest,
    SnapshotStats,
};

use crate::hash::blake3_u64_pair;
use crate::Grid;

/// Blake3 domain tag for snapshot fingerprints. Part of the on-disk
/// contract — changing this byte string invalidates every existing
/// snapshot's dedup key and must be treated as a format migration.
const SNAPSHOT_DOMAIN_TAG: &[u8] = b"nit-gol-snapshot-v1";

/// Two-word blake3 fingerprint of a grid's dimensions and cells.
pub fn grid_fingerprint(grid: &Grid) -> [u64; 2] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SNAPSHOT_DOMAIN_TAG);
    hasher.update(&grid.width().to_le_bytes());
    hasher.update(&grid.height().to_le_bytes());
    hasher.update(grid.cells());
    blake3_u64_pair(&hasher.finalize())
}

/// Pack grid cells into a `u64` bitset for compact snapshot storage.
///
/// Bit `i` of word `i/64` corresponds to cell `i` in row-major order.
/// The layout must agree with `crate::rle::write_rle_bits`, which
/// consumes the packed output.
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

// Test-only imports bridged into module scope for `use super::*;` inside
// the out-of-tree test file. `LastSnapshotKey` / `SnapshotKey` are private
// to `dedup`; tests construct struct literals to probe the gate directly.
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
