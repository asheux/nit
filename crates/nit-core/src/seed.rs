use std::path::Path;

use serde::{Deserialize, Serialize};

use nit_gol::Grid;
use nit_utils::hashing::{stable_hash_bytes, XorShift64};

use crate::config::GolSeedSource;

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedEncoderId {
    AsciiBytes,
    Lifehash16,
    HilbertBits,
}

impl SeedEncoderId {
    pub fn as_str(self) -> &'static str {
        match self {
            SeedEncoderId::AsciiBytes => "ascii_bytes",
            SeedEncoderId::Lifehash16 => "lifehash16",
            SeedEncoderId::HilbertBits => "hilbert_bits",
        }
    }

    pub fn label(self) -> &'static str {
        self.as_str()
    }

    pub fn next(self) -> Self {
        match self {
            SeedEncoderId::AsciiBytes => SeedEncoderId::HilbertBits,
            SeedEncoderId::HilbertBits => SeedEncoderId::Lifehash16,
            SeedEncoderId::Lifehash16 => SeedEncoderId::AsciiBytes,
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedViewMode {
    Genome,
    Plate,
    Map,
    Stats,
}

impl SeedViewMode {
    pub fn next(self) -> Self {
        match self {
            SeedViewMode::Genome => SeedViewMode::Plate,
            SeedViewMode::Plate => SeedViewMode::Map,
            SeedViewMode::Map => SeedViewMode::Stats,
            SeedViewMode::Stats => SeedViewMode::Genome,
        }
    }

    pub fn toggle_plate(self) -> Self {
        match self {
            SeedViewMode::Genome => SeedViewMode::Plate,
            SeedViewMode::Plate => SeedViewMode::Genome,
            other => other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeedViewMode::Genome => "GENOME",
            SeedViewMode::Plate => "PLATE",
            SeedViewMode::Map => "MAP",
            SeedViewMode::Stats => "STATS",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedPreviewMode {
    Solid,
    HalfBlock,
    Braille,
    Tissue,
    Heatmap,
}

impl SeedPreviewMode {
    pub fn next(self) -> Self {
        match self {
            SeedPreviewMode::Solid => SeedPreviewMode::HalfBlock,
            SeedPreviewMode::HalfBlock => SeedPreviewMode::Braille,
            SeedPreviewMode::Braille => SeedPreviewMode::Tissue,
            SeedPreviewMode::Tissue => SeedPreviewMode::Heatmap,
            SeedPreviewMode::Heatmap => SeedPreviewMode::Solid,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeedPreviewMode::Solid => "SOLID",
            SeedPreviewMode::HalfBlock => "HALF",
            SeedPreviewMode::Braille => "BRAILLE",
            SeedPreviewMode::Tissue => "TISSUE",
            SeedPreviewMode::Heatmap => "HEAT",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedSymmetry {
    None,
    MirrorX,
    MirrorY,
    Rotate180,
}

impl SeedSymmetry {
    pub fn label(self) -> &'static str {
        match self {
            SeedSymmetry::None => "none",
            SeedSymmetry::MirrorX => "mirror-x",
            SeedSymmetry::MirrorY => "mirror-y",
            SeedSymmetry::Rotate180 => "rotate-180",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedPlacement {
    Center,
    TopLeft,
}

impl SeedPlacement {
    pub fn label(self) -> &'static str {
        match self {
            SeedPlacement::Center => "center",
            SeedPlacement::TopLeft => "top-left",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeedParams {
    pub symmetry: SeedSymmetry,
    pub target_density: f32,
    pub padding: u8,
    pub placement: SeedPlacement,
    pub jitter: f32,
}

impl Default for SeedParams {
    fn default() -> Self {
        Self {
            symmetry: SeedSymmetry::MirrorX,
            target_density: 0.31,
            padding: 1,
            placement: SeedPlacement::Center,
            jitter: 0.04,
        }
    }
}

impl SeedParams {
    pub fn summary(&self) -> String {
        format!(
            "sym:{} dens:{:.2} pad:{} place:{} jit:{:.2}",
            self.symmetry.label(),
            self.target_density,
            self.padding,
            self.placement.label(),
            self.jitter
        )
    }

    pub fn fingerprint(&self) -> u64 {
        let mut bytes = Vec::with_capacity(16);
        bytes.push(self.symmetry as u8);
        bytes.push(self.placement as u8);
        bytes.extend_from_slice(&(self.target_density.clamp(0.0, 1.0) * 1000.0).round().to_le_bytes());
        bytes.extend_from_slice(&(self.jitter.clamp(0.0, 1.0) * 1000.0).round().to_le_bytes());
        bytes.push(self.padding);
        stable_hash_bytes(&bytes)
    }
}

pub struct SeedInput<'a> {
    pub text: &'a str,
    pub source: GolSeedSource,
    pub file_path: Option<&'a Path>,
    pub version: u64,
}

#[derive(Clone, Debug)]
pub struct SeedValueGrid {
    width: usize,
    height: usize,
    values: Vec<u8>,
}

impl SeedValueGrid {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            values: vec![0; width.saturating_mul(height)],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn values(&self) -> &[u8] {
        &self.values
    }

    pub fn values_mut(&mut self) -> &mut [u8] {
        &mut self.values
    }

    pub fn get(&self, x: usize, y: usize) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.values[y * self.width + x]
    }

    pub fn set(&mut self, x: usize, y: usize, value: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.values[y * self.width + x] = value;
    }
}

#[derive(Clone, Debug)]
pub struct SeedBits {
    width: usize,
    height: usize,
    cells: Vec<u8>,
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
        self.cells[y * self.width + x] = if value { 1 } else { 0 };
    }

    pub fn cells(&self) -> &[u8] {
        &self.cells
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SeedStats {
    pub density: f32,
    pub alive: usize,
    pub components: usize,
    pub base_width: usize,
    pub base_height: usize,
}

#[derive(Clone, Debug)]
pub struct EncodedSeed {
    pub encoder_id: SeedEncoderId,
    pub params: SeedParams,
    pub variant: u8,
    pub input_hash: u64,
    pub seed_hash: u64,
    pub source: GolSeedSource,
    pub base_values: SeedValueGrid,
    pub base_bits: SeedBits,
    pub grid: Grid,
    pub stats: SeedStats,
}

pub trait SeedEncoder {
    fn id(&self) -> SeedEncoderId;
    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid;
}

pub fn encode_seed(
    input: &SeedInput<'_>,
    encoder: SeedEncoderId,
    params: &SeedParams,
    seed_nonce: u64,
    variant: u8,
    target_width: usize,
    target_height: usize,
) -> EncodedSeed {
    let input_hash = stable_hash_bytes(input.text.as_bytes());
    let base_values = match encoder {
        SeedEncoderId::AsciiBytes => AsciiBytesEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::Lifehash16 => Lifehash16Encoder.encode(input, seed_nonce, variant),
        SeedEncoderId::HilbertBits => HilbertBitsEncoder.encode(input, seed_nonce, variant),
    };
    let mut values = base_values.clone();
    apply_jitter(values.values_mut(), params.jitter, input_hash ^ seed_nonce ^ (variant as u64));
    let threshold = density_threshold(params.target_density);
    let mut bits = SeedBits::new(values.width(), values.height());
    for y in 0..values.height() {
        for x in 0..values.width() {
            let alive = values.get(x, y) >= threshold;
            bits.set(x, y, alive);
        }
    }
    apply_symmetry(&mut bits, params.symmetry);
    let seed_hash = hash_seed(encoder, params, variant, &bits);
    let grid = map_bits_to_grid(&bits, target_width, target_height, params);
    let alive = grid.alive_count();
    let total = grid.width().saturating_mul(grid.height()).max(1);
    let density = alive as f32 / total as f32;
    let components = count_components(&grid);
    let stats = SeedStats {
        density,
        alive,
        components,
        base_width: bits.width(),
        base_height: bits.height(),
    };
    EncodedSeed {
        encoder_id: encoder,
        params: params.clone(),
        variant,
        input_hash,
        seed_hash,
        source: input.source,
        base_values: values,
        base_bits: bits,
        grid,
        stats,
    }
}

fn density_threshold(target_density: f32) -> u8 {
    let clamped = target_density.clamp(0.0, 1.0);
    let threshold = (1.0 - clamped) * 255.0;
    threshold.round().clamp(0.0, 255.0) as u8
}

fn hash_seed(encoder: SeedEncoderId, params: &SeedParams, variant: u8, bits: &SeedBits) -> u64 {
    let mut bytes = Vec::with_capacity(bits.cells.len().saturating_add(64));
    bytes.extend_from_slice(encoder.as_str().as_bytes());
    bytes.extend_from_slice(&params.fingerprint().to_le_bytes());
    bytes.push(variant);
    bytes.extend_from_slice(&bits.width.to_le_bytes());
    bytes.extend_from_slice(&bits.height.to_le_bytes());
    bytes.extend_from_slice(bits.cells());
    stable_hash_bytes(&bytes)
}

fn apply_jitter(values: &mut [u8], jitter: f32, seed: u64) {
    let jitter = jitter.clamp(0.0, 1.0);
    if jitter <= f32::EPSILON {
        return;
    }
    let amp = (jitter * 32.0).round() as i16;
    if amp <= 0 {
        return;
    }
    let mut rng = XorShift64::new(seed);
    let span = (amp * 2 + 1) as u64;
    for value in values.iter_mut() {
        let delta = (rng.next_u64() % span) as i16 - amp;
        let next = (*value as i16 + delta).clamp(0, 255) as u8;
        *value = next;
    }
}

fn apply_symmetry(bits: &mut SeedBits, symmetry: SeedSymmetry) {
    let w = bits.width();
    let h = bits.height();
    match symmetry {
        SeedSymmetry::None => {}
        SeedSymmetry::MirrorX => {
            for y in 0..h {
                for x in 0..w / 2 {
                    let alive = bits.get(x, y);
                    let rx = w - 1 - x;
                    if alive {
                        bits.set(rx, y, true);
                    }
                }
            }
        }
        SeedSymmetry::MirrorY => {
            for y in 0..h / 2 {
                for x in 0..w {
                    let alive = bits.get(x, y);
                    let ry = h - 1 - y;
                    if alive {
                        bits.set(x, ry, true);
                    }
                }
            }
        }
        SeedSymmetry::Rotate180 => {
            for y in 0..h {
                for x in 0..w {
                    let alive = bits.get(x, y);
                    if alive {
                        let rx = w - 1 - x;
                        let ry = h - 1 - y;
                        bits.set(rx, ry, true);
                    }
                }
            }
        }
    }
}

fn map_bits_to_grid(bits: &SeedBits, width: usize, height: usize, params: &SeedParams) -> Grid {
    let mut grid = Grid::new(width, height);
    if width == 0 || height == 0 || bits.width() == 0 || bits.height() == 0 {
        return grid;
    }
    let padding = params.padding as usize;
    let avail_w = width.saturating_sub(padding.saturating_mul(2)).max(1);
    let avail_h = height.saturating_sub(padding.saturating_mul(2)).max(1);
    let dest_w = avail_w;
    let dest_h = avail_h;
    let offset_x = match params.placement {
        SeedPlacement::Center => width.saturating_sub(dest_w) / 2,
        SeedPlacement::TopLeft => padding.min(width.saturating_sub(1)),
    };
    let offset_y = match params.placement {
        SeedPlacement::Center => height.saturating_sub(dest_h) / 2,
        SeedPlacement::TopLeft => padding.min(height.saturating_sub(1)),
    };
    for dy in 0..dest_h {
        let by = dy.saturating_mul(bits.height()) / dest_h.max(1);
        for dx in 0..dest_w {
            let bx = dx.saturating_mul(bits.width()) / dest_w.max(1);
            if bits.get(bx, by) {
                let x = offset_x.saturating_add(dx);
                let y = offset_y.saturating_add(dy);
                if x < width && y < height {
                    grid.set(x, y, true);
                }
            }
        }
    }
    grid
}

fn count_components(grid: &Grid) -> usize {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return 0;
    }
    let mut visited = vec![false; w.saturating_mul(h)];
    let mut components = 0usize;
    let mut stack = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if visited[idx] || !grid.get(x, y) {
                continue;
            }
            components += 1;
            visited[idx] = true;
            stack.push((x, y));
            while let Some((cx, cy)) = stack.pop() {
                for ny in cy.saturating_sub(1)..=(cy + 1).min(h - 1) {
                    for nx in cx.saturating_sub(1)..=(cx + 1).min(w - 1) {
                        let nidx = ny * w + nx;
                        if !visited[nidx] && grid.get(nx, ny) {
                            visited[nidx] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
        }
    }
    components
}

struct AsciiBytesEncoder;

impl SeedEncoder for AsciiBytesEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::AsciiBytes
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 32usize;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let mut rng = XorShift64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64));
        let len = bytes.len().max(1);
        for idx in 0..size * size {
            let base = bytes[idx % len];
            let mix = (rng.next_u64() & 0xff) as u8;
            let value = base.wrapping_add((idx as u8).wrapping_mul(31)) ^ mix;
            let x = idx % size;
            let y = idx / size;
            grid.set(x, y, value);
        }
        grid
    }
}

struct Lifehash16Encoder;

impl SeedEncoder for Lifehash16Encoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::Lifehash16
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 16usize;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let mut rng = XorShift64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64) ^ 0x16_u64);
        for idx in 0..size * size {
            let value = (rng.next_u64() & 0xff) as u8;
            let x = idx % size;
            let y = idx / size;
            grid.set(x, y, value);
        }
        grid
    }
}

struct HilbertBitsEncoder;

impl SeedEncoder for HilbertBitsEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::HilbertBits
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let len = bytes.len().max(1);
        let mut rng = XorShift64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64) ^ 0x5eed_u64);
        for idx in 0..size * size {
            let (x, y) = hilbert_index_to_xy(order, idx as u32);
            let base = bytes[idx % len];
            let mix = (rng.next_u64() & 0xff) as u8;
            let value = base ^ mix;
            grid.set(x as usize, y as usize, value);
        }
        grid
    }
}

fn hilbert_index_to_xy(order: u32, index: u32) -> (u32, u32) {
    let mut x = 0u32;
    let mut y = 0u32;
    let mut t = index;
    let mut s = 1u32;
    let n = 1u32 << order;
    while s < n {
        let rx = (t / 2) & 1;
        let ry = (t ^ rx) & 1;
        let (nx, ny) = rot(s, x, y, rx, ry);
        x = nx + s * rx;
        y = ny + s * ry;
        t /= 4;
        s *= 2;
    }
    (x, y)
}

fn rot(n: u32, x: u32, y: u32, rx: u32, ry: u32) -> (u32, u32) {
    if ry == 0 {
        if rx == 1 {
            return (n - 1 - x, n - 1 - y);
        }
        return (y, x);
    }
    (x, y)
}
