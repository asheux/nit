//! Two-dimensional grid of alive/dead cells with FNV-1a fingerprinting.

use crate::hash;

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EdgeMode {
    /// Cells outside the grid are treated as dead.
    Dead,
    /// Opposite edges connect (torus topology).
    Toroid,
}

/// Row-major: cell `(x, y)` lives at index `y * width + x`.
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

    /// [`Grid::set`] normalises writes to `0`/`1`, so [`Grid::hash`] is
    /// stable across any two grids that compare equal under [`PartialEq`].
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
        self.cells[y * self.width + x] != 0
    }

    /// Out-of-bounds writes are silently ignored.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, alive: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.cells[y * self.width + x] = u8::from(alive);
    }

    pub fn clear(&mut self) {
        self.cells.fill(0);
    }

    #[must_use]
    pub fn alive_count(&self) -> usize {
        self.cells.iter().filter(|&&c| c != 0).count()
    }

    /// Width and height are folded in first so two grids with the same
    /// cell pattern but different shapes hash differently. Used as the
    /// fast identity key for attractor detection and snapshot dedup —
    /// must remain stable across versions.
    #[must_use]
    pub fn hash(&self) -> u64 {
        let mut digest = hash::FNV_OFFSET;
        digest = hash::fnv1a(digest, &self.width.to_le_bytes());
        digest = hash::fnv1a(digest, &self.height.to_le_bytes());
        hash::fnv1a(digest, &self.cells)
    }

    /// Top-left overlap is preserved; the rest of the new grid is dead.
    /// Shrinking truncates from the bottom-right; growing pads with dead
    /// cells. The zero-overlap early return guards against `chunks_exact(0)`
    /// panicking when either width is zero.
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
}
