//! Hashing primitives for grid fingerprinting and snapshot dedup.
//!
//! Outputs are stability-critical: any change to a constant, byte order,
//! or the `edge_tag` mapping invalidates on-disk snapshots and
//! in-memory attractor history.

use crate::grid::EdgeMode;

// FNV-1a 64-bit basis and prime; frozen by spec.
pub(crate) const FNV_OFFSET: u64 = 0xcbf29ce484222325;
pub(crate) const FNV_PRIME: u64 = 0x100000001b3;

#[inline]
pub(crate) fn fnv1a(mut digest: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        digest ^= u64::from(byte);
        digest = digest.wrapping_mul(FNV_PRIME);
    }
    digest
}

/// Distinct bytes for `Dead`/`Toroid` so the edge policy folds into a
/// fingerprint without ambiguity.
#[inline]
pub(crate) fn edge_tag(edge: EdgeMode) -> u8 {
    match edge {
        EdgeMode::Dead => 0,
        EdgeMode::Toroid => 1,
    }
}

/// Two little-endian `u64` halves of a blake3 digest — primary identity
/// plus secondary collision-guard, both from a single blake3 evaluation.
pub(crate) fn blake3_u64_pair(hash: &blake3::Hash) -> [u64; 2] {
    let bytes = hash.as_bytes();
    let lo = u64::from_le_bytes(
        bytes[0..8]
            .try_into()
            .expect("blake3 digest is 32 bytes wide"),
    );
    let hi = u64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .expect("blake3 digest is 32 bytes wide"),
    );
    [lo, hi]
}

#[inline]
pub(crate) fn blake3_u64(hash: &blake3::Hash) -> u64 {
    blake3_u64_pair(hash)[0]
}
