use nit_gol::Grid;
use nit_utils::hashing::stable_hash_bytes;
use nit_utils::rng::SplitMix64;

use super::grid_types::{Grid2D, SeedBits, SeedValueGrid};
use super::params::{SeedParams, SeedPlacement, SeedSymmetry};
use super::view_modes::SeedEncoderId;

impl<T: Copy + Default> Grid2D<T> {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![T::default(); width.saturating_mul(height)],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn data(&self) -> &[T] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [T] {
        &mut self.data
    }

    pub fn get(&self, x: usize, y: usize) -> T {
        if x >= self.width || y >= self.height {
            return T::default();
        }
        self.data[y * self.width + x]
    }

    pub fn set(&mut self, x: usize, y: usize, value: T) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.data[y * self.width + x] = value;
    }
}

impl SeedBits {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0; width.saturating_mul(height)],
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
        self.cells[y * self.width + x] != 0
    }

    pub fn set(&mut self, x: usize, y: usize, value: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.cells[y * self.width + x] = u8::from(value);
    }
}

pub(super) fn density_threshold(target_density: f32) -> u8 {
    let clamped = target_density.clamp(0.0, 1.0);
    let threshold = (1.0 - clamped) * 255.0;
    threshold.round().clamp(0.0, 255.0) as u8
}

pub(super) fn hash_seed(
    encoder: SeedEncoderId,
    params: &SeedParams,
    variant: u8,
    bits: &SeedBits,
) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(encoder.as_str().as_bytes());
    hasher.update(&params.fingerprint().to_le_bytes());
    hasher.update(&[variant]);
    hasher.update(&bits.width().to_le_bytes());
    hasher.update(&bits.height().to_le_bytes());
    hasher.update(bits.cells());
    let hash = hasher.finalize();
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
}

pub(super) fn apply_jitter(values: &mut [u8], jitter: f32, seed: u64) {
    let jitter = jitter.clamp(0.0, 1.0);
    if jitter <= f32::EPSILON {
        return;
    }
    let amp = (jitter * 32.0).round() as i16;
    if amp <= 0 {
        return;
    }
    let mut rng = SplitMix64::new(seed);
    let span = (amp * 2 + 1) as u64;
    for value in values.iter_mut() {
        let delta = ((rng.next_u64() >> 48) % span) as i16 - amp;
        let next = (*value as i16 + delta).clamp(0, 255) as u8;
        *value = next;
    }
}

pub(super) fn apply_symmetry(bits: &mut SeedBits, symmetry: SeedSymmetry) {
    match symmetry {
        SeedSymmetry::None => {}
        SeedSymmetry::MirrorX => mirror_horizontally(bits),
        SeedSymmetry::MirrorY => mirror_vertically(bits),
        SeedSymmetry::Rotate180 => rotate_half_turn(bits),
    }
}

fn mirror_horizontally(bits: &mut SeedBits) {
    let (w, h) = (bits.width(), bits.height());
    for y in 0..h {
        for x in 0..w / 2 {
            let rx = w - 1 - x;
            let alive = bits.get(x, y) || bits.get(rx, y);
            bits.set(x, y, alive);
            bits.set(rx, y, alive);
        }
    }
}

fn mirror_vertically(bits: &mut SeedBits) {
    let (w, h) = (bits.width(), bits.height());
    for y in 0..h / 2 {
        let ry = h - 1 - y;
        for x in 0..w {
            let alive = bits.get(x, y) || bits.get(x, ry);
            bits.set(x, y, alive);
            bits.set(x, ry, alive);
        }
    }
}

fn rotate_half_turn(bits: &mut SeedBits) {
    let (w, h) = (bits.width(), bits.height());
    for y in 0..h {
        for x in 0..w {
            let rx = w - 1 - x;
            let ry = h - 1 - y;
            let alive = bits.get(x, y) || bits.get(rx, ry);
            bits.set(x, y, alive);
            bits.set(rx, ry, alive);
        }
    }
}

struct GridLayout {
    offset_x: usize,
    offset_y: usize,
    dest_w: usize,
    dest_h: usize,
}

pub(super) fn map_bits_to_grid(
    bits: &SeedBits,
    width: usize,
    height: usize,
    params: &SeedParams,
) -> Grid {
    let mut grid = Grid::new(width, height);
    if width == 0 || height == 0 || bits.width() == 0 || bits.height() == 0 {
        return grid;
    }
    let layout = compute_grid_layout(width, height, params);
    blit_bits_into_grid(bits, &mut grid, width, height, &layout);
    grid
}

fn compute_grid_layout(width: usize, height: usize, params: &SeedParams) -> GridLayout {
    let padding = params.padding as usize;
    let dest_w = width.saturating_sub(padding.saturating_mul(2)).max(1);
    let dest_h = height.saturating_sub(padding.saturating_mul(2)).max(1);
    let (offset_x, offset_y) = match params.placement {
        SeedPlacement::Center => (
            width.saturating_sub(dest_w) / 2,
            height.saturating_sub(dest_h) / 2,
        ),
        SeedPlacement::TopLeft => (
            padding.min(width.saturating_sub(1)),
            padding.min(height.saturating_sub(1)),
        ),
    };
    GridLayout {
        offset_x,
        offset_y,
        dest_w,
        dest_h,
    }
}

fn blit_bits_into_grid(
    bits: &SeedBits,
    grid: &mut Grid,
    width: usize,
    height: usize,
    layout: &GridLayout,
) {
    for dy in 0..layout.dest_h {
        let by = dy.saturating_mul(bits.height()) / layout.dest_h.max(1);
        for dx in 0..layout.dest_w {
            let bx = dx.saturating_mul(bits.width()) / layout.dest_w.max(1);
            if !bits.get(bx, by) {
                continue;
            }
            let x = layout.offset_x.saturating_add(dx);
            let y = layout.offset_y.saturating_add(dy);
            if x < width && y < height {
                grid.set(x, y, true);
            }
        }
    }
}

// 8-connectivity (Moore neighborhood): diagonally adjacent alive cells are
// part of the same component.
pub(super) fn count_components(grid: &Grid) -> usize {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return 0;
    }
    let mut visited = vec![false; w.saturating_mul(h)];
    let mut components = 0usize;
    for y in 0..h {
        for x in 0..w {
            if visited[y * w + x] || !grid.get(x, y) {
                continue;
            }
            components += 1;
            flood_fill_alive(grid, &mut visited, w, h, x, y);
        }
    }
    components
}

fn flood_fill_alive(
    grid: &Grid,
    visited: &mut [bool],
    w: usize,
    h: usize,
    start_x: usize,
    start_y: usize,
) {
    visited[start_y * w + start_x] = true;
    let mut stack = vec![(start_x, start_y)];
    while let Some((cx, cy)) = stack.pop() {
        for ny in cy.saturating_sub(1)..=(cy + 1).min(h - 1) {
            for nx in cx.saturating_sub(1)..=(cx + 1).min(w - 1) {
                let nidx = ny * w + nx;
                if visited[nidx] || !grid.get(nx, ny) {
                    continue;
                }
                visited[nidx] = true;
                stack.push((nx, ny));
            }
        }
    }
}

// Span the full 0-255 range so the density threshold works correctly
// regardless of an encoder's raw value distribution.
pub(super) fn normalize_grid(grid: &mut SeedValueGrid) {
    let values = grid.data();
    if values.is_empty() {
        return;
    }
    let min_val = values.iter().copied().min().unwrap_or(0);
    let max_val = values.iter().copied().max().unwrap_or(0);
    if min_val == max_val {
        return;
    }
    let range = (max_val - min_val) as f32;
    for v in grid.data_mut() {
        *v = ((*v - min_val) as f32 / range * 255.0).round() as u8;
    }
}

// Tiny per-cell perturbation so that two encodings of structurally equivalent
// inputs don't collapse to the exact same grid.
pub(super) fn apply_structural_noise(
    grid: &mut SeedValueGrid,
    size: usize,
    seed_nonce: u64,
    bytes: &[u8],
    variant: u8,
) {
    let total = size * size;
    let mut rng =
        SplitMix64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64) ^ 0x57ac_u64);
    for idx in 0..total {
        let x = idx % size;
        let y = idx / size;
        let base = grid.get(x, y) as i16;
        let noise = ((rng.next_u64() >> 56) as i16).wrapping_sub(128) / 10;
        grid.set(x, y, (base + noise).clamp(0, 255) as u8);
    }
}

pub(super) fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (idx, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

pub(super) fn hilbert_index_to_xy(order: u32, index: u32) -> (u32, u32) {
    let mut x = 0u32;
    let mut y = 0u32;
    let mut t = index;
    let mut s = 1u32;
    let n = 1u32 << order;
    while s < n {
        let rx = (t / 2) & 1;
        let ry = (t ^ rx) & 1;
        let (nx, ny) = hilbert_rotate(s, x, y, rx, ry);
        x = nx + s * rx;
        y = ny + s * ry;
        t /= 4;
        s *= 2;
    }
    (x, y)
}

fn hilbert_rotate(n: u32, x: u32, y: u32, rx: u32, ry: u32) -> (u32, u32) {
    if ry != 0 {
        return (x, y);
    }
    if rx == 1 {
        return (n - 1 - x, n - 1 - y);
    }
    (y, x)
}
