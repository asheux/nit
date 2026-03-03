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
    let cell_w = grid_w.div_ceil(2);
    let cell_h = grid_h.div_ceil(4);
    let w = cell_w.min(max_w);
    let h = cell_h.min(max_h);

    for y in 0..h {
        for x in 0..w {
            let mut top_alive = false;
            let mut bottom_alive = false;
            let mut top_color = palette.live;
            let mut bottom_color = palette.live;
            let mut top_color_set = false;
            let mut bottom_color_set = false;
            let mut top_halo = 0u8;
            let mut bottom_halo = 0u8;

            for dy in 0..4 {
                let gy = y * 4 + dy;
                if gy >= grid_h {
                    continue;
                }
                for dx in 0..2 {
                    let gx = x * 2 + dx;
                    if gx >= grid_w {
                        continue;
                    }
                    if seed.grid.get(gx, gy) {
                        if dy < 2 {
                            top_alive = true;
                            if !top_color_set {
                                top_color = live_color(gx, gy, seed, cfg, cache, palette);
                                top_color_set = true;
                            }
                        } else {
                            bottom_alive = true;
                            if !bottom_color_set {
                                bottom_color = live_color(gx, gy, seed, cfg, cache, palette);
                                bottom_color_set = true;
                            }
                        }
                    } else if cfg.show_halo {
                        if let Some(halo) = &cache.halo_mask {
                            let idx = gy * grid_w + gx;
                            if idx < halo.len() {
                                if dy < 2 {
                                    top_halo = top_halo.max(halo[idx]);
                                } else {
                                    bottom_halo = bottom_halo.max(halo[idx]);
                                }
                            }
                        }
                    }
                }
            }

            let top_bg = if top_alive {
                top_color
            } else if cfg.show_halo && top_halo > 0 {
                halo_color(top_halo, palette)
            } else {
                palette.bg
            };
            let bottom_bg = if bottom_alive {
                bottom_color
            } else if cfg.show_halo && bottom_halo > 0 {
                halo_color(bottom_halo, palette)
            } else {
                palette.bg
            };

            let (ch, fg, bg) = match (top_alive, bottom_alive) {
                (true, true) => {
                    if top_bg == bottom_bg {
                        ('█', top_bg, top_bg)
                    } else {
                        ('▀', top_bg, bottom_bg)
                    }
                }
                (true, false) => ('▀', top_bg, bottom_bg),
                (false, true) => ('▄', bottom_bg, top_bg),
                (false, false) => {
                    let halo = top_halo.max(bottom_halo);
                    let bg = if cfg.show_halo && halo > 0 {
                        halo_color(halo, palette)
                    } else {
                        palette.bg
                    };
                    (' ', palette.bg, bg)
                }
            };
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            cell.set_char(ch);
            cell.set_style(Style::default().fg(fg).bg(bg));
        }
    }
}
