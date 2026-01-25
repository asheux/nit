use blake3::Hasher;

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
        let mut hasher = Hasher::new();
        hasher.update(&self.width.to_le_bytes());
        hasher.update(&self.height.to_le_bytes());
        hasher.update(&self.cells);
        let mut out = [0u8; 8];
        out.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
        u64::from_le_bytes(out)
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
