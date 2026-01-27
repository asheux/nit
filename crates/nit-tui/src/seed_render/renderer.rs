use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use nit_core::seed::SeedBits;
use nit_core::{EncodedSeed, SeedEncoderId, SeedPreviewMode};
use nit_gol::Grid;

use super::palette::SeedPalette;
use super::{braille, halfblock, heatmap, overlays, solid, tissue};

#[derive(Clone, Debug)]
pub struct SeedRenderConfig {
    pub mode: SeedPreviewMode,
    pub show_grid: bool,
    pub show_bbox: bool,
    pub show_halo: bool,
    pub show_components: bool,
    pub show_inset_genome: bool,
    pub scanline: bool,
    pub zoom: u8,
}

#[derive(Clone, Debug, Default)]
pub struct BBox {
    pub id: u16,
    pub min_x: usize,
    pub min_y: usize,
    pub max_x: usize,
    pub max_y: usize,
    pub cells: u32,
}

#[derive(Clone, Debug)]
pub struct SeedRenderCache {
    pub seed_hash: u64,
    pub grid_width: usize,
    pub grid_height: usize,
    pub genome_live: usize,
    pub genome_total: usize,
    pub genome_density: f32,
    pub ascii_printable: usize,
    pub ascii_nonprintable: usize,
    pub component_map: Option<Vec<u16>>,
    pub component_bboxes: Vec<BBox>,
    pub local_density: Option<Vec<u8>>,
    pub density_stride: usize,
    pub density_block: usize,
    pub halo_mask: Option<Vec<u8>>,
    pub inset_16x16: Option<SeedBits>,
    pub hilbert_stream: Option<Vec<u8>>,
    pub hilbert_index_by_xy: Option<Vec<u32>>,
    pub hilbert_path_inset: Option<Vec<u8>>,
    pub hilbert_order: u32,
    pub scanline_phase: u16,
}

impl Default for SeedRenderCache {
    fn default() -> Self {
        Self {
            seed_hash: 0,
            grid_width: 0,
            grid_height: 0,
            genome_live: 0,
            genome_total: 0,
            genome_density: 0.0,
            ascii_printable: 0,
            ascii_nonprintable: 0,
            component_map: None,
            component_bboxes: Vec::new(),
            local_density: None,
            density_stride: 0,
            density_block: 4,
            halo_mask: None,
            inset_16x16: None,
            hilbert_stream: None,
            hilbert_index_by_xy: None,
            hilbert_path_inset: None,
            hilbert_order: 0,
            scanline_phase: 0,
        }
    }
}

impl SeedRenderCache {
    pub fn update(&mut self, seed: &EncodedSeed) {
        let w = seed.grid.width();
        let h = seed.grid.height();
        if self.seed_hash == seed.seed_hash && self.grid_width == w && self.grid_height == h {
            return;
        }
        self.seed_hash = seed.seed_hash;
        self.grid_width = w;
        self.grid_height = h;

        let (component_map, mut bboxes) = compute_components(&seed.grid);
        bboxes.sort_by(|a, b| b.cells.cmp(&a.cells));
        self.component_map = component_map;
        self.component_bboxes = bboxes;

        let (density, stride, block) = compute_density(&seed.grid, self.density_block.max(2));
        self.local_density = density;
        self.density_stride = stride;
        self.density_block = block;

        self.halo_mask = compute_halo(&seed.grid);

        self.inset_16x16 = if seed.encoder_id.as_str() == "lifehash16"
            && seed.base_bits_raw.width() == 16
            && seed.base_bits_raw.height() == 16
        {
            Some(seed.base_bits_raw.clone())
        } else {
            None
        };

        self.hilbert_stream = None;
        self.hilbert_index_by_xy = None;
        self.hilbert_path_inset = None;
        self.hilbert_order = 0;
        if seed.encoder_id == SeedEncoderId::HilbertBits {
            let w = seed.base_bits.width().max(1);
            let h = seed.base_bits.height().max(1);
            let total = w.saturating_mul(h);
            let mut order = 0u32;
            let mut size = 1usize;
            while size < w {
                size <<= 1;
                order += 1;
            }
            let mut stream = vec![0u8; total];
            let mut index_by_xy = vec![0u32; total];
            let mut inset = vec![0u8; 16 * 16];
            let denom = total.saturating_sub(1).max(1) as u32;
            for idx in 0..total {
                let (x, y) = hilbert_index_to_xy(order, idx as u32);
                let xi = x as usize;
                let yi = y as usize;
                if xi < w && yi < h {
                    stream[idx] = if seed.base_bits.get(xi, yi) { 1 } else { 0 };
                    index_by_xy[yi * w + xi] = idx as u32;
                    let ix = xi.saturating_mul(16) / w;
                    let iy = yi.saturating_mul(16) / h;
                    let v = ((idx as u32).saturating_mul(255) / denom) as u8;
                    let inset_idx = iy.saturating_mul(16) + ix;
                    if inset_idx < inset.len() && v > inset[inset_idx] {
                        inset[inset_idx] = v;
                    }
                }
            }
            self.hilbert_stream = Some(stream);
            self.hilbert_index_by_xy = Some(index_by_xy);
            self.hilbert_path_inset = Some(inset);
            self.hilbert_order = order;
        }

        let mut live = 0usize;
        let mut total = 0usize;
        for y in 0..seed.base_bits_raw.height() {
            for x in 0..seed.base_bits_raw.width() {
                total += 1;
                if seed.base_bits_raw.get(x, y) {
                    live += 1;
                }
            }
        }
        self.genome_live = live;
        self.genome_total = total.max(1);
        self.genome_density = live as f32 / self.genome_total as f32;

        let mut printable = 0usize;
        let mut nonprintable = 0usize;
        for y in 0..seed.base_values.height() {
            for x in 0..seed.base_values.width() {
                let v = seed.base_values.get(x, y);
                if v >= 0x20 && v <= 0x7e {
                    printable += 1;
                } else {
                    nonprintable += 1;
                }
            }
        }
        self.ascii_printable = printable;
        self.ascii_nonprintable = nonprintable;
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

pub fn grid_size_for_mode(width: usize, height: usize, mode: SeedPreviewMode) -> (usize, usize) {
    match mode {
        SeedPreviewMode::HalfBlock => (width, height.saturating_mul(2)),
        SeedPreviewMode::Braille => (width.saturating_mul(2), height.saturating_mul(4)),
        SeedPreviewMode::Solid | SeedPreviewMode::Tissue | SeedPreviewMode::Heatmap => {
            (width, height)
        }
    }
}

pub fn render_seed(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    match cfg.mode {
        SeedPreviewMode::Solid => {
            solid::render(area, buf, seed, cfg, cache, palette);
        }
        SeedPreviewMode::HalfBlock => {
            halfblock::render(area, buf, seed, cfg, cache, palette);
        }
        SeedPreviewMode::Braille => {
            braille::render(area, buf, seed, cfg, cache, palette);
        }
        SeedPreviewMode::Tissue => {
            tissue::render(area, buf, seed, cfg, cache, palette);
        }
        SeedPreviewMode::Heatmap => {
            heatmap::render(area, buf, seed, cfg, cache, palette);
        }
    }

    overlays::render_overlays(area, buf, seed, cfg, cache, palette);
}

pub fn live_color(
    x: usize,
    y: usize,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) -> ratatui::style::Color {
    if cfg.show_components {
        if let Some(map) = &cache.component_map {
            let idx = y * seed.grid.width() + x;
            if idx < map.len() {
                let id = map[idx];
                if id != u16::MAX {
                    if let Some(color) = palette.tissue.get(id as usize % palette.tissue.len()) {
                        return *color;
                    }
                }
            }
        }
    }
    palette.live
}

fn compute_components(grid: &Grid) -> (Option<Vec<u16>>, Vec<BBox>) {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return (None, Vec::new());
    }
    let mut map = vec![u16::MAX; w * h];
    let mut bboxes = Vec::new();
    let mut stack = Vec::new();
    let mut next_id: u16 = 0;

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if grid.cells()[idx] == 0 || map[idx] != u16::MAX {
                continue;
            }
            if next_id == u16::MAX {
                break;
            }
            let mut bbox = BBox {
                id: next_id,
                min_x: x,
                min_y: y,
                max_x: x,
                max_y: y,
                cells: 0,
            };
            map[idx] = next_id;
            stack.push((x, y));
            while let Some((cx, cy)) = stack.pop() {
                bbox.cells = bbox.cells.saturating_add(1);
                bbox.min_x = bbox.min_x.min(cx);
                bbox.min_y = bbox.min_y.min(cy);
                bbox.max_x = bbox.max_x.max(cx);
                bbox.max_y = bbox.max_y.max(cy);
                let x0 = cx.saturating_sub(1);
                let x1 = (cx + 1).min(w - 1);
                let y0 = cy.saturating_sub(1);
                let y1 = (cy + 1).min(h - 1);
                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let nidx = ny * w + nx;
                        if grid.cells()[nidx] == 0 {
                            continue;
                        }
                        if map[nidx] == u16::MAX {
                            map[nidx] = next_id;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            bboxes.push(bbox);
            next_id = next_id.wrapping_add(1);
        }
    }

    if bboxes.is_empty() {
        (None, Vec::new())
    } else {
        (Some(map), bboxes)
    }
}

fn compute_density(grid: &Grid, block: usize) -> (Option<Vec<u8>>, usize, usize) {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return (None, 0, block);
    }
    let block = block.max(2);
    let bw = (w + block - 1) / block;
    let bh = (h + block - 1) / block;
    let mut density = vec![0u8; bw * bh];
    for y in 0..h {
        for x in 0..w {
            if grid.get(x, y) {
                let bx = x / block;
                let by = y / block;
                let idx = by * bw + bx;
                density[idx] = density[idx].saturating_add(1);
            }
        }
    }
    (Some(density), bw, block)
}

fn compute_halo(grid: &Grid) -> Option<Vec<u8>> {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return None;
    }
    let mut halo = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            if !grid.get(x, y) {
                continue;
            }
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    if nx == x && ny == y {
                        continue;
                    }
                    if grid.get(nx, ny) {
                        continue;
                    }
                    let idx = ny * w + nx;
                    halo[idx] = halo[idx].saturating_add(1);
                }
            }
        }
    }
    Some(halo)
}

pub fn halo_color(intensity: u8, palette: &SeedPalette) -> ratatui::style::Color {
    if intensity >= 3 {
        palette.halo_2
    } else {
        palette.halo_1
    }
}

pub fn base_style(bg: ratatui::style::Color) -> Style {
    Style::default().bg(bg)
}
