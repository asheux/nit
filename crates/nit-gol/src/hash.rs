//! Hashing primitives for grid fingerprinting and deduplication.
//!
//! All outputs are deterministic and must remain stable across versions —
//! they participate in attractor cycle detection and snapshot dedup keys.

use crate::grid::EdgeMode;

pub(crate) const FNV_OFFSET: u64 = 0xcbf29ce484222325;
pub(crate) const FNV_PRIME: u64 = 0x100000001b3;

/// Feed a byte slice into a running FNV-1a hash.
///
/// Stability-critical: participates in attractor detection and snapshot
/// deduplication. Do not alter the algorithm.
pub(crate) fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Map an [`EdgeMode`] to a deterministic byte tag so that grids evolved
/// under different boundary policies produce distinct fingerprints.
pub(crate) fn edge_tag(edge: EdgeMode) -> u8 {
    match edge {
        EdgeMode::Dead => 0,
        EdgeMode::Toroid => 1,
    }
}

/// Extract a `[u64; 2]` pair from the first 16 bytes of a blake3 digest.
pub(crate) fn blake3_u64_pair(hash: &blake3::Hash) -> [u64; 2] {
    let bytes = hash.as_bytes();
    [read_u64_le(bytes, 0), read_u64_le(bytes, 8)]
}

/// Extract a single `u64` from the first 8 bytes of a blake3 digest.
pub(crate) fn blake3_u64(hash: &blake3::Hash) -> u64 {
    read_u64_le(hash.as_bytes(), 0)
}

fn read_u64_le(bytes: &[u8], offset: usize) -> u64 {
    let slice: [u8; 8] = bytes[offset..offset + 8]
        .try_into()
        .expect("blake3 hash always has 32 bytes");
    u64::from_le_bytes(slice)
}
