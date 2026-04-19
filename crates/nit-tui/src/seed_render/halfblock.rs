use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use nit_core::EncodedSeed;

use super::cache_compute::SeedRenderCache;
use super::paint::{halo_bg_at, write_glyph};
use super::palette::SeedPalette;
use super::renderer::{live_color, SeedRenderConfig, SeedRenderer};

pub(super) struct HalfBlockSeedRenderer;

impl SeedRenderer for HalfBlockSeedRenderer {
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
        let cell_w = grid_w.min(area.width as usize);
        let cell_h = grid_h.div_ceil(2).min(area.height as usize);

        for y in 0..cell_h {
            let top_y = y * 2;
            let bot_y = top_y + 1;
            for x in 0..cell_w {
                let top = sample_row(x, top_y, grid_w, grid_h, seed, cfg, cache, palette);
                let bot = sample_row(x, bot_y, grid_w, grid_h, seed, cfg, cache, palette);
                let (ch, fg, bg) = compose(top, bot, palette.bg);
                write_glyph(
                    buf,
                    area.x + x as u16,
                    area.y + y as u16,
                    ch,
                    Style::default().fg(fg).bg(bg),
                );
            }
        }
    }
}

struct RowSample {
    alive: bool,
    bg: Color,
}

#[allow(clippy::too_many_arguments)]
fn sample_row(
    x: usize,
    y: usize,
    grid_w: usize,
    grid_h: usize,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) -> RowSample {
    if y >= grid_h {
        return RowSample {
            alive: false,
            bg: palette.bg,
        };
    }
    if seed.grid.get(x, y) {
        return RowSample {
            alive: true,
            bg: live_color(x, y, seed, cfg, cache, palette),
        };
    }
    let bg = if cfg.show_halo {
        halo_bg_at(x, y, grid_w, cache, palette)
    } else {
        palette.bg
    };
    RowSample { alive: false, bg }
}

fn compose(top: RowSample, bot: RowSample, empty_bg: Color) -> (char, Color, Color) {
    match (top.alive, bot.alive) {
        (true, true) if top.bg == bot.bg => ('█', top.bg, top.bg),
        (true, true) | (true, false) => ('▀', top.bg, bot.bg),
        (false, true) => ('▄', bot.bg, top.bg),
        (false, false) => (' ', empty_bg, empty_bg),
    }
}
