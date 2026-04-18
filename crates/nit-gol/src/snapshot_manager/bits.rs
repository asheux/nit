//! Grid fingerprinting and bit-packing utilities used by snapshot writes.

use crate::hash::blake3_u64_pair;
use crate::Grid;

/// Two-word blake3 fingerprint of a grid's dimensions and cells.
///
/// The domain tag `nit-gol-snapshot-v1` is part of the on-disk
/// fingerprint contract — changing it invalidates existing snapshots.
pub fn grid_fingerprint(grid: &Grid) -> [u64; 2] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"nit-gol-snapshot-v1");
    hasher.update(&grid.width().to_le_bytes());
    hasher.update(&grid.height().to_le_bytes());
    hasher.update(grid.cells());
    blake3_u64_pair(&hasher.finalize())
}

/// Pack grid cells into a `u64` bitset for compact snapshot storage.
///
/// Bit `i` of word `i/64` corresponds to cell `i` in row-major order.
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
