//! Content-hash-based deduplication and cooldown enforcement.

use std::time::{Duration, Instant};

use super::types::{SnapshotEventKind, SnapshotRequest};
use crate::hash::blake3_u64;

/// Content signature of a snapshot request. Two requests that produce
/// the same key are considered duplicates and collapse to a single
/// write. Fields must stay in-line with the `from_request` mapping;
/// their `Hash`/`Eq` derivation is load-bearing for the dedup gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SnapshotKey {
    pub(super) event_kind: SnapshotEventKind,
    pub(super) rule_hash: u64,
    pub(super) seed_hash: u64,
    pub(super) grid_hash: [u64; 2],
    pub(super) period: Option<u64>,
}

impl SnapshotKey {
    pub(super) fn from_request(req: &SnapshotRequest) -> Self {
        Self {
            event_kind: req.event,
            rule_hash: rule_hash(&req.rule),
            seed_hash: req.seed_hash,
            grid_hash: req.grid_hash,
            period: req.period,
        }
    }
}

/// The most recently admitted key, plus the instant it was admitted.
pub(super) struct LastSnapshotKey {
    pub(super) key: Option<SnapshotKey>,
    pub(super) last_at: Instant,
}

impl LastSnapshotKey {
    /// Decide whether a new request should be admitted.
    ///
    /// Test-pinned ordering: the dedup check runs first, so Manual
    /// events bypass the cooldown window but still collapse against
    /// an identical most-recent key.
    pub(super) fn allows(
        &self,
        key: &SnapshotKey,
        event_kind: SnapshotEventKind,
        now: Instant,
        min_interval: Duration,
    ) -> bool {
        if self.is_duplicate(key) {
            return false;
        }
        if matches!(event_kind, SnapshotEventKind::Manual) {
            return true;
        }
        now.duration_since(self.last_at) >= min_interval
    }

    fn is_duplicate(&self, key: &SnapshotKey) -> bool {
        self.key.as_ref() == Some(key)
    }
}

fn rule_hash(rule: &str) -> u64 {
    blake3_u64(&blake3::hash(rule.as_bytes()))
}
