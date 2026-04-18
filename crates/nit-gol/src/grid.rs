//! Grid representation for cellular automata.
//!
//! A `Grid` is a two-dimensional byte array where each cell is `0` (dead)
//! or `1` (alive). The grid supports boundary-checked access, population
//! counting, and FNV-1a hashing for fast identity comparison.

use crate::hash;

/// Boundary handling policy for the simulation grid.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EdgeMode {
    /// Cells outside the grid are treated as dead.
    Dead,
    /// Opposite edges are connected (torus topology).
    Toroid,
}

/// A two-dimensional grid of alive/dead cells.
///
/// Storage is row-major: cell `(x, y)` lives at index `y * width + x`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    width: usize,
    height: usize,
    cells: Vec<u8>,
}

impl Grid {
    /// All cells start dead.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height);
        Self {
            width,
            height,
            cells: vec![0; len],
        }
    }

    #[inline]
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[inline]
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Raw cell storage (row-major, one byte per cell, `0` dead / `1` alive).
    ///
    /// [`Grid::set`] normalizes writes to `0`/`1`, so [`Grid::hash`] is
    /// stable for any two grids that compare equal under [`PartialEq`].
    #[must_use]
    pub fn cells(&self) -> &[u8] {
        &self.cells
    }

    /// Out-of-bounds reads return `false`.
    #[inline]
    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.cells[self.index(x, y)] != 0
    }

    /// Out-of-bounds writes are silently ignored.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, alive: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = self.index(x, y);
        self.cells[idx] = u8::from(alive);
    }

    pub fn clear(&mut self) {
        self.cells.fill(0);
    }

    #[must_use]
    pub fn alive_count(&self) -> usize {
        self.cells.iter().filter(|&&c| c != 0).count()
    }

    /// 64-bit FNV-1a hash over dimensions and cell data.
    ///
    /// Deterministic across versions; used as a fast identity key for
    /// attractor detection and snapshot deduplication. Width and height
    /// are folded in first so two grids with the same cell pattern but
    /// different shapes hash differently.
    #[must_use]
    pub fn hash(&self) -> u64 {
        let mut digest = hash::FNV_OFFSET;
        digest = hash::fnv1a(digest, &self.width.to_le_bytes());
        digest = hash::fnv1a(digest, &self.height.to_le_bytes());
        hash::fnv1a(digest, &self.cells)
    }

    /// Copy this grid into a new grid of different dimensions.
    ///
    /// Cells within the top-left overlapping region are preserved; cells
    /// outside that region in the new grid are dead. Shrinking truncates
    /// from the bottom-right; growing pads with dead cells.
    #[must_use]
    pub fn clone_with_size(&self, width: usize, height: usize) -> Grid {
        let mut resized = Grid::new(width, height);
        let overlap_width = width.min(self.width);
        let overlap_height = height.min(self.height);
        if overlap_width == 0 || overlap_height == 0 {
            return resized;
        }
        let src_rows = self.cells.chunks_exact(self.width).take(overlap_height);
        let dst_rows = resized.cells.chunks_exact_mut(width).take(overlap_height);
        for (src, dst) in src_rows.zip(dst_rows) {
            dst[..overlap_width].copy_from_slice(&src[..overlap_width]);
        }
        resized
    }

    #[inline]
    fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }
}
