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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    width: usize,
    height: usize,
    cells: Vec<u8>,
}

impl Grid {
    /// Create an all-dead grid with the given dimensions.
    pub fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height);
        Self {
            width,
            height,
            cells: vec![0; len],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Raw cell storage (row-major, one byte per cell).
    pub fn cells(&self) -> &[u8] {
        &self.cells
    }

    /// Returns `true` if the cell at `(x, y)` is alive.
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.cells[self.index(x, y)] != 0
    }

    /// Set the cell at `(x, y)` to alive or dead.
    pub fn set(&mut self, x: usize, y: usize, alive: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = self.index(x, y);
        self.cells[idx] = u8::from(alive);
    }

    /// Reset all cells to dead.
    pub fn clear(&mut self) {
        self.cells.fill(0);
    }

    /// Count the number of alive cells in the grid.
    pub fn alive_count(&self) -> usize {
        self.cells.iter().map(|v| *v as usize).sum()
    }

    /// Compute a 64-bit FNV-1a hash over dimensions and cell data.
    ///
    /// This hash is used for fast grid-equality checks in the rule
    /// evaluator and must remain deterministic across versions.
    pub fn hash(&self) -> u64 {
        let mut h = hash::FNV_OFFSET;
        h = hash::fnv1a(h, &self.width.to_le_bytes());
        h = hash::fnv1a(h, &self.height.to_le_bytes());
        hash::fnv1a(h, &self.cells)
    }

    /// Copy this grid into a new grid with different dimensions.
    ///
    /// Cells within the overlapping region are preserved; new cells
    /// are initialized to dead.
    pub fn clone_with_size(&self, width: usize, height: usize) -> Grid {
        let mut new_grid = Grid::new(width, height);
        let copy_w = width.min(self.width);
        let copy_h = height.min(self.height);
        for y in 0..copy_h {
            for x in 0..copy_w {
                new_grid.set(x, y, self.get(x, y));
            }
        }
        new_grid
    }

    /// Convert `(x, y)` to a flat index into the cell storage.
    fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }
}
