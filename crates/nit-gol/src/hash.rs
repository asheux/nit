//! Hashing primitives for grid fingerprinting and deduplication.
//!
//! Outputs are stability-critical: changing any constant, byte order,
//! or the `edge_tag` mapping invalidates on-disk snapshots and
//! in-memory attractor history.

use crate::grid::EdgeMode;

// Standard FNV-1a 64-bit basis and prime; frozen by design.
pub(crate) const FNV_OFFSET: u64 = 0xcbf29ce484222325;
pub(crate) const FNV_PRIME: u64 = 0x100000001b3;

/// Fold `bytes` into `digest` using FNV-1a 64.
#[inline]
pub(crate) fn fnv1a(mut digest: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        digest ^= u64::from(byte);
        digest = digest.wrapping_mul(FNV_PRIME);
    }
    digest
}

/// Encode the edge-wrap policy as a single byte so it can be folded
/// into a fingerprint without ambiguity between `Dead` and `Toroid`.
#[inline]
pub(crate) fn edge_tag(edge: EdgeMode) -> u8 {
    match edge {
        EdgeMode::Dead => 0,
        EdgeMode::Toroid => 1,
    }
}

/// Split a blake3 digest into two little-endian `u64` halves.
///
/// Used by the attractor detector to get both a primary identity hash
/// and a secondary collision-guard hash from a single blake3 evaluation.
pub(crate) fn blake3_u64_pair(hash: &blake3::Hash) -> [u64; 2] {
    let bytes = hash.as_bytes();
    [read_u64_le(bytes, 0), read_u64_le(bytes, 8)]
}

pub(crate) fn blake3_u64(hash: &blake3::Hash) -> u64 {
    read_u64_le(hash.as_bytes(), 0)
}

#[inline]
fn read_u64_le(bytes: &[u8; 32], offset: usize) -> u64 {
    let chunk: [u8; 8] = bytes[offset..offset + 8]
        .try_into()
        .expect("offset + 8 within 32-byte hash");
    u64::from_le_bytes(chunk)
}
