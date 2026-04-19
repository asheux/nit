use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use nit_core::EncodedSeed;

use super::cache_compute::SeedRenderCache;
use super::palette::SeedPalette;
use super::renderer::{SeedRenderConfig, SeedRenderer};
use super::solid;

// Tissue shares Solid's pixel geometry — the per-component palette is dispatched inside
// `renderer::live_color` when `cfg.show_components` is set.
pub(super) struct TissueSeedRenderer;

impl SeedRenderer for TissueSeedRenderer {
    fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        seed: &EncodedSeed,
        cfg: &SeedRenderConfig,
        cache: &SeedRenderCache,
        palette: &SeedPalette,
    ) {
        solid::render_cell_grid(area, buf, seed, cfg, cache, palette);
    }
}
