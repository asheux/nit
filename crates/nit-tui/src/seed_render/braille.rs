use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use nit_core::EncodedSeed;

use super::cache_compute::SeedRenderCache;
use super::paint::{halo_color, write_glyph};
use super::palette::SeedPalette;
use super::renderer::{live_color, SeedRenderConfig, SeedRenderer};

const CELL_W_PX: usize = 2;
const CELL_H_PX: usize = 4;

pub(super) struct BrailleSeedRenderer;

impl SeedRenderer for BrailleSeedRenderer {
    fn render(
        &self,
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
}

struct HalfSample {
    // None while no live pixel has been seen for this half — the first live pixel's
    // color wins, matching pre-refactor behavior where subsequent pixels were ignored.
    live_color: Option<Color>,
    halo: u8,
}

impl HalfSample {
    fn empty() -> Self {
        Self {
            live_color: None,
            halo: 0,
        }
    }

    fn alive(&self) -> bool {
        self.live_color.is_some()
    }

    fn bg(&self, show_halo: bool, palette: &SeedPalette) -> Color {
        if let Some(color) = self.live_color {
            color
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
    let mut halves = CellHalves {
        top: HalfSample::empty(),
        bottom: HalfSample::empty(),
    };

    for dy in 0..CELL_H_PX {
        let gy = cell_y * CELL_H_PX + dy;
        if gy >= grid_h {
            continue;
        }
        let half = if dy < CELL_H_PX / 2 {
            &mut halves.top
        } else {
            &mut halves.bottom
        };
        for dx in 0..CELL_W_PX {
            let gx = cell_x * CELL_W_PX + dx;
            if gx < grid_w {
                sample_pixel(half, gx, gy, seed, cfg, cache, palette);
            }
        }
    }

    halves
}

// Fold one grid pixel into the half-sample. Live pixels contribute the first color we see
// (to match pre-refactor "first wins" semantics); dead pixels contribute halo intensity
// when the overlay is enabled.
fn sample_pixel(
    half: &mut HalfSample,
    gx: usize,
    gy: usize,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    if seed.grid.get(gx, gy) {
        half.live_color
            .get_or_insert_with(|| live_color(gx, gy, seed, cfg, cache, palette));
        return;
    }
    if !cfg.show_halo {
        return;
    }
    let Some(mask) = cache.halo_mask.as_deref() else {
        return;
    };
    let Some(&v) = mask.get(gy * seed.grid.width() + gx) else {
        return;
    };
    half.halo = half.halo.max(v);
}

fn compose_glyph(
    sample: &CellHalves,
    show_halo: bool,
    palette: &SeedPalette,
) -> (char, Color, Color) {
    let top_bg = sample.top.bg(show_halo, palette);
    let bot_bg = sample.bottom.bg(show_halo, palette);
    match (sample.top.alive(), sample.bottom.alive()) {
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
