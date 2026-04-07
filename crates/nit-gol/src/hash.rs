//! Hashing primitives for grid fingerprinting and deduplication.
//!
//! Provides FNV-1a and blake3 extraction utilities used across the crate
//! for grid identity checks, attractor detection, and snapshot dedup.
//! All hash outputs are deterministic and must remain stable across
//! versions — they participate in attractor cycle detection and
//! snapshot deduplication protocols.

use crate::grid::EdgeMode;

/// FNV-1a 64-bit offset basis.
///
/// Standard starting state for the Fowler–Noll–Vo hash computation.
pub(crate) const FNV_OFFSET: u64 = 0xcbf29ce484222325;

/// FNV-1a 64-bit prime multiplier.
///
/// Applied after each XOR step to diffuse bit patterns across
/// the full 64-bit hash state.
pub(crate) const FNV_PRIME: u64 = 0x100000001b3;

/// Feed a byte slice into a running FNV-1a hash.
///
/// Processes each byte by XOR-ing it into `hash` then multiplying
/// by [`FNV_PRIME`]. Returns the updated accumulator.
///
/// # Stability
///
/// Output values participate in attractor detection and snapshot
/// deduplication. Do not alter the algorithm.
pub(crate) fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Map an [`EdgeMode`] to a deterministic byte tag for hashing.
///
/// Encodes `Dead` as `0` and `Toroid` as `1` so that grids evolved
/// under different boundary policies produce distinct fingerprints.
pub(crate) fn edge_tag(edge: EdgeMode) -> u8 {
    match edge {
        EdgeMode::Dead => 0,
        EdgeMode::Toroid => 1,
    }
}

/// Extract a `[u64; 2]` pair from the first 16 bytes of a blake3 digest.
///
/// Produces a compact two-word fingerprint used for grid identity
/// checks in both attractor detection and snapshot management.
pub(crate) fn blake3_u64_pair(hash: &blake3::Hash) -> [u64; 2] {
    let bytes = hash.as_bytes();
    let lo = u64::from_le_bytes(
        bytes[0..8]
            .try_into()
            .expect("blake3 hash always has 32 bytes"),
    );
    let hi = u64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .expect("blake3 hash always has 32 bytes"),
    );
    [lo, hi]
}

/// Extract a single `u64` from the first 8 bytes of a blake3 digest.
///
/// Useful when only one word of entropy is needed, such as
/// rule-string hashing for snapshot deduplication keys.
pub(crate) fn blake3_u64(hash: &blake3::Hash) -> u64 {
    let bytes = hash.as_bytes();
    u64::from_le_bytes(
        bytes[0..8]
            .try_into()
            .expect("blake3 hash always has 32 bytes"),
    )
}
