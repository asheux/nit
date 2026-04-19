use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use nit_core::EncodedSeed;

use super::paint::{bg_style, halo_bg_at};
use super::palette::SeedPalette;
use super::renderer::{SeedRenderCache, SeedRenderConfig, live_color};

pub fn render(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    render_cell_grid(area, buf, seed, cfg, cache, palette);
}

// Shared cell-grid renderer used by Solid and Tissue preview modes. Tissue dispatches a
// per-component palette through `live_color` when `cfg.show_components` is set, so this
// function handles both modes with identical pixel geometry.
pub(super) fn render_cell_grid(
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
    let w = grid_w.min(area.width as usize);
    let h = grid_h.min(area.height as usize);

    for y in 0..h {
        for x in 0..w {
            paint_cell(area, buf, x, y, grid_w, seed, cfg, cache, palette);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_cell(
    area: Rect,
    buf: &mut Buffer,
    x: usize,
    y: usize,
    grid_w: usize,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
    if seed.grid.get(x, y) {
        let fg = live_color(x, y, seed, cfg, cache, palette);
        cell.set_char('▀');
        cell.set_style(Style::default().fg(fg).bg(palette.bg));
    } else {
        cell.set_char(' ');
        cell.set_style(bg_style(dead_bg(x, y, grid_w, cfg, cache, palette)));
    }
}

fn dead_bg(
    x: usize,
    y: usize,
    grid_w: usize,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) -> Color {
    if cfg.show_halo {
        halo_bg_at(x, y, grid_w, cache, palette)
    } else {
        palette.bg
    }
}
