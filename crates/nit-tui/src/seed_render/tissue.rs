use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use nit_core::EncodedSeed;

use super::palette::SeedPalette;
use super::renderer::{base_style, halo_color, live_color, SeedRenderCache, SeedRenderConfig};

pub fn render(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
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

    for y in 0..h {
        for x in 0..w {
            let alive = seed.grid.get(x, y);
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            if alive {
                let fg = live_color(x, y, seed, cfg, cache, palette);
                cell.set_char('▀');
                cell.set_style(Style::default().fg(fg).bg(palette.bg));
            } else {
                let mut bg = palette.bg;
                if cfg.show_halo {
                    if let Some(halo) = &cache.halo_mask {
                        let idx = y * grid_w + x;
                        if idx < halo.len() && halo[idx] > 0 {
                            bg = halo_color(halo[idx], palette);
                        }
                    }
                }
                cell.set_char(' ');
                cell.set_style(base_style(bg));
            }
        }
    }
}
