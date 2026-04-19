use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use nit_core::{EncodedSeed, SeedPreviewMode};

use super::cache_compute::SeedRenderCache;
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

// Stateless renderer for one preview mode. Implementors read from `seed`
// and the precomputed `cache`, and write a single frame into `buf`.
pub(super) trait SeedRenderer {
    fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        seed: &EncodedSeed,
        cfg: &SeedRenderConfig,
        cache: &SeedRenderCache,
        palette: &SeedPalette,
    );
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
    dispatch_mode(cfg.mode).render(area, buf, seed, cfg, cache, palette);
    overlays::render_overlays(area, buf, seed, cfg, cache, palette);
}

fn dispatch_mode(mode: SeedPreviewMode) -> &'static dyn SeedRenderer {
    match mode {
        SeedPreviewMode::Solid => &solid::SolidSeedRenderer,
        SeedPreviewMode::HalfBlock => &halfblock::HalfBlockSeedRenderer,
        SeedPreviewMode::Braille => &braille::BrailleSeedRenderer,
        SeedPreviewMode::Tissue => &tissue::TissueSeedRenderer,
        SeedPreviewMode::Heatmap => &heatmap::HeatmapSeedRenderer,
    }
}

pub(super) fn live_color(
    x: usize,
    y: usize,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) -> ratatui::style::Color {
    if !cfg.show_components {
        return palette.live;
    }
    let Some(map) = cache.component_map.as_ref() else {
        return palette.live;
    };
    let idx = y * seed.grid.width() + x;
    let Some(&id) = map.get(idx) else {
        return palette.live;
    };
    if id == u16::MAX {
        return palette.live;
    }
    palette
        .tissue
        .get(id as usize % palette.tissue.len())
        .copied()
        .unwrap_or(palette.live)
}
