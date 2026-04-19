use nit_core::{GolRenderMode, VisualizerMode};
use nit_gol::Grid;

pub const MAX_AGE: u8 = 12;
pub const MAX_DECAY: u8 = 10;
pub const HUD_HISTORY_LEN: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct GolRenderConfig {
    pub mode: GolRenderMode,
    pub age_shading: bool,
    pub trails: bool,
    pub overlay_bbox: bool,
    pub overlay_heat: bool,
    pub scanlines: bool,
    pub grid_minor: Option<u16>,
    pub grid_major: Option<u16>,
    pub gol_origin_x: i32,
    pub gol_origin_y: i32,
    pub debug_overlay: bool,
    pub braille_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct GolRenderState {
    width: usize,
    height: usize,
    pub(crate) age: Vec<u8>,
    pub(crate) decay: Vec<u8>,
    pub hud: GolHudMetrics,
}

impl Default for GolRenderState {
    fn default() -> Self {
        Self::new()
    }
}

impl GolRenderState {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            age: Vec::new(),
            decay: Vec::new(),
            hud: GolHudMetrics::new(HUD_HISTORY_LEN),
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        let len = width.saturating_mul(height);
        self.age.resize(len, 0);
        self.decay.resize(len, 0);
        self.hud.history.head = 0;
        self.hud.history.filled = false;
        self.hud.delta = 0;
    }

    pub fn seed_from_grid(&mut self, grid: &Grid) -> usize {
        self.resize(grid.width(), grid.height());
        let mut alive = 0usize;
        for (idx, &cell) in grid.cells().iter().enumerate() {
            let is_alive = cell != 0;
            self.age[idx] = u8::from(is_alive);
            self.decay[idx] = 0;
            if is_alive {
                alive += 1;
            }
        }
        self.hud.history.head = 0;
        self.hud.history.filled = false;
        self.hud.delta = 0;
        self.hud.history.push(alive.min(u16::MAX as usize) as u16);
        alive
    }

    pub fn update_from_step(&mut self, prev: &Grid, next: &Grid) -> (usize, u32) {
        let width = next.width();
        let height = next.height();
        if self.width != width || self.height != height {
            self.resize(width, height);
        }
        let prev_cells = prev.cells();
        let next_cells = next.cells();
        let mut alive = 0usize;
        let mut delta = 0u32;
        for idx in 0..next_cells.len() {
            let was_alive = prev_cells[idx] != 0;
            let is_alive = next_cells[idx] != 0;
            if was_alive != is_alive {
                delta = delta.saturating_add(1);
            }
            if is_alive {
                alive += 1;
                let age = self.age[idx];
                self.age[idx] = if was_alive {
                    age.saturating_add(1).min(MAX_AGE)
                } else {
                    1
                };
                self.decay[idx] = 0;
                continue;
            }
            self.age[idx] = 0;
            self.decay[idx] = if was_alive {
                MAX_DECAY
            } else {
                self.decay[idx].saturating_sub(1)
            };
        }
        self.hud.delta = delta;
        self.hud.history.push(alive.min(u16::MAX as usize) as u16);
        (alive, delta)
    }
}

#[derive(Clone, Debug)]
pub struct GolHudMetrics {
    pub history: AliveHistory,
    pub delta: u32,
}

impl GolHudMetrics {
    pub fn new(history_len: usize) -> Self {
        Self {
            history: AliveHistory::new(history_len),
            delta: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AliveHistory {
    buf: Vec<u16>,
    head: usize,
    filled: bool,
}

impl AliveHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0; capacity],
            head: 0,
            filled: false,
        }
    }

    pub fn push(&mut self, value: u16) {
        if self.buf.is_empty() {
            return;
        }
        self.buf[self.head] = value;
        self.head = (self.head + 1) % self.buf.len();
        if self.head == 0 {
            self.filled = true;
        }
    }

    pub fn len(&self) -> usize {
        if self.filled {
            self.buf.len()
        } else {
            self.head
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, idx: usize) -> Option<u16> {
        let len = self.len();
        if idx >= len {
            return None;
        }
        if !self.filled {
            return Some(self.buf[idx]);
        }
        let pos = (self.head + idx) % self.buf.len();
        Some(self.buf[pos])
    }
}

pub struct GolHudState<'a> {
    pub rule: &'a str,
    pub generation: u64,
    pub alive: usize,
    pub period: Option<u32>,
    pub mode: VisualizerMode,
    pub paused: bool,
    pub delta: u32,
    pub history: &'a AliveHistory,
}
