use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use nit_core::EncodedSeed;

use super::cache_compute::SeedRenderCache;
use super::palette::SeedPalette;
use super::renderer::{SeedRenderConfig, SeedRenderer};

const HEAT_CHARS: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

pub(super) struct HeatmapSeedRenderer;

impl SeedRenderer for HeatmapSeedRenderer {
    // `_cfg` is intentionally unused: heatmap ignores overlay toggles so the dispatch
    // signature stays uniform with the other modes.
    fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        seed: &EncodedSeed,
        _cfg: &SeedRenderConfig,
        cache: &SeedRenderCache,
        palette: &SeedPalette,
    ) {
        let grid_w = seed.grid.width();
        let grid_h = seed.grid.height();
        if grid_w == 0 || grid_h == 0 {
            return;
        }
        let Some(density) = cache.local_density.as_ref() else {
            return;
        };
        let w = grid_w.min(area.width as usize);
        let h = grid_h.min(area.height as usize);
        let params = DensityParams::from_cache(cache);

        for y in 0..h {
            for x in 0..w {
                let count = params.sample(density, x, y);
                let (ch, fg) = heat_glyph(count, &params, palette);
                let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
                cell.set_char(ch);
                cell.set_style(Style::default().fg(fg).bg(palette.bg));
            }
        }
    }
}

struct DensityParams {
    stride: usize,
    block: usize,
    max_density: u8,
}

impl DensityParams {
    fn from_cache(cache: &SeedRenderCache) -> Self {
        let stride = cache.density_stride.max(1);
        let block = cache.density_block.max(2);
        Self {
            stride,
            block,
            max_density: (block * block) as u8,
        }
    }

    fn sample(&self, density: &[u8], x: usize, y: usize) -> u8 {
        let bx = x / self.block;
        let by = y / self.block;
        density.get(by * self.stride + bx).copied().unwrap_or(0)
    }
}

fn heat_glyph(count: u8, params: &DensityParams, palette: &SeedPalette) -> (char, Color) {
    let top_idx = HEAT_CHARS.len() - 1;
    let level = (count as usize * top_idx) / params.max_density.max(1) as usize;
    let ch = HEAT_CHARS[level.min(top_idx)];
    let half_max = params.max_density / 2;
    let fg = if count == 0 {
        palette.hud_dim
    } else if count < half_max {
        palette.live_dim
    } else {
        palette.live
    };
    (ch, fg)
}
