use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use nit_core::EncodedSeed;

use super::palette::SeedPalette;
use super::renderer::{SeedRenderCache, SeedRenderConfig};
use super::solid;

// Tissue previews share the pixel-grid layout with Solid mode; tissue palette dispatch
// happens inside `renderer::live_color` when `cfg.show_components` is set.
pub fn render(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    solid::render_cell_grid(area, buf, seed, cfg, cache, palette);
}
