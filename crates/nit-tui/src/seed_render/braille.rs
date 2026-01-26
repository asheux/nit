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
    let cell_w = (grid_w + 1) / 2;
    let cell_h = (grid_h + 3) / 4;
    let w = cell_w.min(max_w);
    let h = cell_h.min(max_h);

    for y in 0..h {
        for x in 0..w {
            let mut mask = 0u8;
            let mut color = palette.live;
            let mut color_set = false;
            let mut halo_max = 0u8;
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
                        mask |= braille_bit(dx, dy);
                        if !color_set {
                            color = live_color(gx, gy, seed, cfg, cache, palette);
                            color_set = true;
                        }
                    } else if cfg.show_halo {
                        if let Some(halo) = &cache.halo_mask {
                            let idx = gy * grid_w + gx;
                            if idx < halo.len() {
                                halo_max = halo_max.max(halo[idx]);
                            }
                        }
                    }
                }
            }
            let (ch, fg, bg) = if mask == 0 {
                let bg = if cfg.show_halo && halo_max > 0 {
                    halo_color(halo_max, palette)
                } else {
                    palette.bg
                };
                (' ', palette.bg, bg)
            } else {
                (braille_char(mask), color, palette.bg)
            };
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            cell.set_char(ch);
            cell.set_style(Style::default().fg(fg).bg(bg));
        }
    }
}

fn braille_char(mask: u8) -> char {
    char::from_u32(0x2800 + mask as u32).unwrap_or(' ')
}

fn braille_bit(dx: usize, dy: usize) -> u8 {
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (0, 3) => 0x40,
        (1, 3) => 0x80,
        _ => 0x00,
    }
}
