use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use nit_core::{EncodedSeed, SeedPreviewMode};
use nit_core::seed::SeedBits;
use nit_gol::Grid;

use super::{braille, halfblock, heatmap, overlays, solid, tissue};
use super::palette::SeedPalette;

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
    pub component_map: Option<Vec<u16>>,
    pub component_bboxes: Vec<BBox>,
    pub local_density: Option<Vec<u8>>,
    pub density_stride: usize,
    pub density_block: usize,
    pub halo_mask: Option<Vec<u8>>,
    pub inset_16x16: Option<SeedBits>,
    pub scanline_phase: u16,
}

impl Default for SeedRenderCache {
    fn default() -> Self {
        Self {
            seed_hash: 0,
            grid_width: 0,
            grid_height: 0,
            component_map: None,
            component_bboxes: Vec::new(),
            local_density: None,
            density_stride: 0,
            density_block: 4,
            halo_mask: None,
            inset_16x16: None,
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
            && seed.base_bits.width() == 16
            && seed.base_bits.height() == 16
        {
            Some(seed.base_bits.clone())
        } else {
            None
        };
    }
}

pub fn grid_size_for_mode(width: usize, height: usize, mode: SeedPreviewMode) -> (usize, usize) {
    match mode {
        SeedPreviewMode::HalfBlock => (width, height.saturating_mul(2)),
        SeedPreviewMode::Braille => (width.saturating_mul(2), height.saturating_mul(4)),
        SeedPreviewMode::BitGrid
        | SeedPreviewMode::Tissue
        | SeedPreviewMode::Heatmap
        | SeedPreviewMode::Matrix
        | SeedPreviewMode::Motif => (width, height),
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
        SeedPreviewMode::BitGrid => {
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
        SeedPreviewMode::Matrix | SeedPreviewMode::Motif => {}
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
