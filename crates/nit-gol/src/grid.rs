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
    /// Create an all-dead grid with the given dimensions.
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

    /// Raw cell storage (row-major, one byte per cell).
    #[must_use]
    pub fn cells(&self) -> &[u8] {
        &self.cells
    }

    /// Read the cell at `(x, y)`. Out-of-bounds coordinates read as dead.
    #[inline]
    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.cells[self.index(x, y)] != 0
    }

    /// Write the cell at `(x, y)`. Out-of-bounds writes are silently ignored.
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
        let mut h = hash::FNV_OFFSET;
        h = hash::fnv1a(h, &self.width.to_le_bytes());
        h = hash::fnv1a(h, &self.height.to_le_bytes());
        hash::fnv1a(h, &self.cells)
    }

    /// Copy this grid into a new grid of different dimensions.
    ///
    /// Cells within the overlapping region (top-left anchored) are
    /// preserved; cells outside that region in the new grid are dead.
    #[must_use]
    pub fn clone_with_size(&self, width: usize, height: usize) -> Grid {
        let mut new_grid = Grid::new(width, height);
        let copy_w = width.min(self.width);
        let copy_h = height.min(self.height);
        if copy_w == 0 || copy_h == 0 {
            return new_grid;
        }
        for y in 0..copy_h {
            let src_start = y * self.width;
            let dst_start = y * width;
            new_grid.cells[dst_start..dst_start + copy_w]
                .copy_from_slice(&self.cells[src_start..src_start + copy_w]);
        }
        new_grid
    }

    #[inline]
    fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }
}
