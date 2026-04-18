//! Blake3-based grid fingerprinting for attractor detection.
//!
//! Byte order and domain tags here are stability-critical — changing
//! any of them invalidates persisted attractor history and snapshot
//! dedup keys already on disk.

use super::AttractorExtra;
use crate::hash::{blake3_u64_pair, edge_tag, fnv1a, FNV_OFFSET};
use crate::{EdgeMode, Grid, Rule};

const FINGERPRINT_DOMAIN: &[u8] = b"nit-gol-attractor-v1";
const PROTOCOL_TAG: &[u8] = b"proto";

/// Two-word blake3-based fingerprint for grid identity.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint(pub(super) [u64; 2]);

/// Compute a primary fingerprint without a secondary collision-guard hash.
pub(super) fn compute(
    grid: &Grid,
    rule: Rule,
    edge: EdgeMode,
    extra: Option<AttractorExtra>,
) -> Fingerprint {
    compute_with_secondary(grid, rule, edge, extra, false).0
}

/// Compute a blake3 primary fingerprint and an optional FNV-1a
/// secondary hash for collision confirmation.
pub(super) fn compute_with_secondary(
    grid: &Grid,
    rule: Rule,
    edge: EdgeMode,
    extra: Option<AttractorExtra>,
    include_secondary: bool,
) -> (Fingerprint, Option<u64>) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(FINGERPRINT_DOMAIN);
    feed_payload(grid, rule, edge, extra, |bytes| {
        hasher.update(bytes);
    });
    let fp = Fingerprint(blake3_u64_pair(&hasher.finalize()));
    let secondary = include_secondary.then(|| secondary_hash(grid, rule, edge, extra));
    (fp, secondary)
}

fn secondary_hash(grid: &Grid, rule: Rule, edge: EdgeMode, extra: Option<AttractorExtra>) -> u64 {
    let mut hash = FNV_OFFSET;
    feed_payload(grid, rule, edge, extra, |bytes| {
        hash = fnv1a(hash, bytes);
    });
    hash
}

/// Emit the canonical payload bytes for both primary and secondary hashes.
///
/// Stability-critical: the byte order here is part of the on-disk
/// fingerprint contract. Any reordering or field addition invalidates
/// persisted attractor history and snapshot dedup keys.
fn feed_payload(
    grid: &Grid,
    rule: Rule,
    edge: EdgeMode,
    extra: Option<AttractorExtra>,
    mut push: impl FnMut(&[u8]),
) {
    push(&grid.width().to_le_bytes());
    push(&grid.height().to_le_bytes());
    push(&rule.births_mask().to_le_bytes());
    push(&rule.survives_mask().to_le_bytes());
    push(&[edge_tag(edge)]);
    if let Some(extra) = extra {
        push(PROTOCOL_TAG);
        push(&extra.protocol_hash.to_le_bytes());
        push(&extra.phase_idx.to_le_bytes());
        push(&extra.step_in_phase.to_le_bytes());
    }
    push(grid.cells());
}

#[cfg(test)]
impl Fingerprint {
    pub(crate) fn from_u128(value: u128) -> Self {
        let bytes = value.to_le_bytes();
        let lo = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let hi = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        Fingerprint([lo, hi])
    }
}
