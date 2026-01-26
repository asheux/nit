use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use nit_core::EncodedSeed;

use super::palette::SeedPalette;
use super::renderer::{halo_color, live_color, SeedRenderCache, SeedRenderConfig};

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
    let cell_w = grid_w.min(max_w);
    let cell_h = (grid_h + 1) / 2;
    let h = cell_h.min(max_h);

    for y in 0..h {
        let top_y = y * 2;
        let bot_y = top_y + 1;
        for x in 0..cell_w {
            let top_alive = top_y < grid_h && seed.grid.get(x, top_y);
            let bot_alive = bot_y < grid_h && seed.grid.get(x, bot_y);

            let mut top_bg = palette.bg;
            let mut bot_bg = palette.bg;
            if top_alive {
                top_bg = live_color(x, top_y, seed, cfg, cache, palette);
            } else if cfg.show_halo {
                if let Some(halo) = &cache.halo_mask {
                    let idx = top_y * grid_w + x;
                    if idx < halo.len() && halo[idx] > 0 {
                        top_bg = halo_color(halo[idx], palette);
                    }
                }
            }
            if bot_alive {
                bot_bg = live_color(x, bot_y, seed, cfg, cache, palette);
            } else if cfg.show_halo {
                if let Some(halo) = &cache.halo_mask {
                    if bot_y < grid_h {
                        let idx = bot_y * grid_w + x;
                        if idx < halo.len() && halo[idx] > 0 {
                            bot_bg = halo_color(halo[idx], palette);
                        }
                    }
                }
            }

            let (ch, fg, bg) = match (top_alive, bot_alive) {
                (true, true) => {
                    if top_bg == bot_bg {
                        ('█', top_bg, top_bg)
                    } else {
                        ('▀', top_bg, bot_bg)
                    }
                }
                (true, false) => ('▀', top_bg, bot_bg),
                (false, true) => ('▄', bot_bg, top_bg),
                (false, false) => (' ', palette.bg, palette.bg),
            };

            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            cell.set_char(ch);
            cell.set_style(Style::default().fg(fg).bg(bg));
        }
    }
}
