use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use nit_core::EncodedSeed;

use super::palette::SeedPalette;
use super::renderer::{SeedRenderCache, SeedRenderConfig};

const HEAT_CHARS: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

pub fn render(
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
    let max_w = area.width as usize;
    let max_h = area.height as usize;
    let w = grid_w.min(max_w);
    let h = grid_h.min(max_h);

    let Some(density) = &cache.local_density else {
        return;
    };
    let stride = cache.density_stride.max(1);
    let block = cache.density_block.max(2);
    let max_density = (block * block) as u8;

    for y in 0..h {
        for x in 0..w {
            let bx = x / block;
            let by = y / block;
            let idx = by * stride + bx;
            let count = density.get(idx).copied().unwrap_or(0);
            let level = (count as usize * (HEAT_CHARS.len() - 1)) / max_density.max(1) as usize;
            let ch = HEAT_CHARS[level.min(HEAT_CHARS.len() - 1)];
            let fg = if count == 0 {
                palette.hud_dim
            } else if count < max_density / 2 {
                palette.live_dim
            } else {
                palette.live
            };
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            cell.set_char(ch);
            cell.set_style(Style::default().fg(fg).bg(palette.bg));
        }
    }
}
