#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EdgeMode {
    Dead,
    Toroid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    width: usize,
    height: usize,
    cells: Vec<u8>,
}

impl Grid {
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

    pub fn cells(&self) -> &[u8] {
        &self.cells
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.cells[self.index(x, y)] != 0
    }

    pub fn set(&mut self, x: usize, y: usize, alive: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = self.index(x, y);
        self.cells[idx] = if alive { 1 } else { 0 };
    }

    pub fn clear(&mut self) {
        self.cells.fill(0);
    }

    pub fn alive_count(&self) -> usize {
        self.cells.iter().map(|v| *v as usize).sum()
    }

    pub fn hash(&self) -> u64 {
        // Simple 64-bit FNV-1a over dimensions + cells. Fast, stable, and low-stack.
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;
        let mut hash = FNV_OFFSET;
        hash = fnv1a(hash, &self.width.to_le_bytes());
        hash = fnv1a(hash, &self.height.to_le_bytes());
        for &byte in &self.cells {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }

    pub fn clone_with_size(&self, width: usize, height: usize) -> Grid {
        let mut new_grid = Grid::new(width, height);
        let min_w = width.min(self.width);
        let min_h = height.min(self.height);
        for y in 0..min_h {
            for x in 0..min_w {
                let alive = self.get(x, y);
                new_grid.set(x, y, alive);
            }
        }
        new_grid
    }

    fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
