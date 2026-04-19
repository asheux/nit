use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use nit_core::EncodedSeed;

use super::paint::{halo_color, write_glyph};
use super::palette::SeedPalette;
use super::renderer::{SeedRenderCache, SeedRenderConfig, live_color};

const CELL_W_PX: usize = 2;
const CELL_H_PX: usize = 4;

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
    let cell_cols = grid_w.div_ceil(CELL_W_PX).min(area.width as usize);
    let cell_rows = grid_h.div_ceil(CELL_H_PX).min(area.height as usize);

    for cell_y in 0..cell_rows {
        for cell_x in 0..cell_cols {
            let sample = sample_cell(cell_x, cell_y, seed, cfg, cache, palette);
            let (ch, fg, bg) = compose_glyph(&sample, cfg.show_halo, palette);
            write_glyph(
                buf,
                area.x + cell_x as u16,
                area.y + cell_y as u16,
                ch,
                Style::default().fg(fg).bg(bg),
            );
        }
    }
}

struct HalfSample {
    alive: bool,
    color: Color,
    color_set: bool,
    halo: u8,
}

impl HalfSample {
    fn bg(&self, show_halo: bool, palette: &SeedPalette) -> Color {
        if self.alive {
            self.color
        } else if show_halo && self.halo > 0 {
            halo_color(self.halo, palette)
        } else {
            palette.bg
        }
    }
}

struct CellHalves {
    top: HalfSample,
    bottom: HalfSample,
}

fn sample_cell(
    cell_x: usize,
    cell_y: usize,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) -> CellHalves {
    let grid_w = seed.grid.width();
    let grid_h = seed.grid.height();
    let mut top = HalfSample {
        alive: false,
        color: palette.live,
        color_set: false,
        halo: 0,
    };
    let mut bottom = HalfSample {
        alive: false,
        color: palette.live,
        color_set: false,
        halo: 0,
    };
    let halo_mask = cache.halo_mask.as_deref();

    for dy in 0..CELL_H_PX {
        let gy = cell_y * CELL_H_PX + dy;
        if gy >= grid_h {
            continue;
        }
        let half = if dy < CELL_H_PX / 2 {
            &mut top
        } else {
            &mut bottom
        };
        for dx in 0..CELL_W_PX {
            let gx = cell_x * CELL_W_PX + dx;
            if gx >= grid_w {
                continue;
            }
            if seed.grid.get(gx, gy) {
                half.alive = true;
                if !half.color_set {
                    half.color = live_color(gx, gy, seed, cfg, cache, palette);
                    half.color_set = true;
                }
                continue;
            }
            if !cfg.show_halo {
                continue;
            }
            if let Some(mask) = halo_mask {
                if let Some(&v) = mask.get(gy * grid_w + gx) {
                    half.halo = half.halo.max(v);
                }
            }
        }
    }

    CellHalves { top, bottom }
}

fn compose_glyph(
    sample: &CellHalves,
    show_halo: bool,
    palette: &SeedPalette,
) -> (char, Color, Color) {
    let top_bg = sample.top.bg(show_halo, palette);
    let bot_bg = sample.bottom.bg(show_halo, palette);
    match (sample.top.alive, sample.bottom.alive) {
        (true, true) if top_bg == bot_bg => ('█', top_bg, top_bg),
        (true, true) | (true, false) => ('▀', top_bg, bot_bg),
        (false, true) => ('▄', bot_bg, top_bg),
        (false, false) => {
            let halo = sample.top.halo.max(sample.bottom.halo);
            let bg = if show_halo && halo > 0 {
                halo_color(halo, palette)
            } else {
                palette.bg
            };
            (' ', palette.bg, bg)
        }
    }
}
