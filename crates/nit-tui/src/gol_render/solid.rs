use ratatui::{buffer::Buffer, layout::Rect};

use nit_gol::Grid;

use super::palette::GolPalette;
use super::renderer::{
    draw_bbox, live_color, neighbor_count, render_hud_line, row_bg, trail_color, GolHudState,
    GolRenderConfig, GolRenderState, GolRenderer,
};

#[derive(Default)]
pub struct SolidRenderer;

impl GolRenderer for SolidRenderer {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        grid: &Grid,
        state: &GolRenderState,
        cfg: &GolRenderConfig,
        palette: &GolPalette,
        hud: &GolHudState<'_>,
    ) {
        render_hud_line(area, buf, palette, hud);
        let grid_area = Rect {
            x: area.x,
            y: area.y.saturating_add(1),
            width: area.width,
            height: area.height.saturating_sub(1),
        };
        if grid_area.width == 0 || grid_area.height == 0 {
            return;
        }

        let grid_w = grid.width();
        let grid_h = grid.height();
        let cells = grid.cells();
        let age = state.age();
        let decay = state.decay();

        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        let mut any_alive = false;

        for row in 0..grid_area.height as usize {
            let y = grid_area.y + row as u16;
            let bg = row_bg(row, cfg, palette);
            let in_row = row < grid_h;
            let row_start = if in_row { row.saturating_mul(grid_w) } else { 0 };
            for col in 0..grid_area.width as usize {
                let x = grid_area.x + col as u16;
                let mut alive = false;
                let mut age_val = 0u8;
                let mut decay_val = 0u8;
                if in_row && col < grid_w {
                    let idx = row_start + col;
                    alive = cells[idx] != 0;
                    age_val = age[idx];
                    decay_val = decay[idx];
                }

                if alive {
                    any_alive = true;
                    min_x = min_x.min(col);
                    min_y = min_y.min(row);
                    max_x = max_x.max(col);
                    max_y = max_y.max(row);
                }

                let color = if alive {
                    let neighbors = if cfg.overlay_heat {
                        neighbor_count(grid, col, row)
                    } else {
                        0
                    };
                    live_color(age_val, neighbors, cfg, palette)
                } else if cfg.trails && decay_val > 0 {
                    trail_color(decay_val, palette)
                } else {
                    bg
                };

                let cell = buf.get_mut(x, y);
                cell.set_char(' ');
                cell.set_bg(color);
                cell.set_fg(color);
            }
        }

        if cfg.overlay_bbox && any_alive {
            draw_bbox(grid_area, buf, min_x, min_y, max_x, max_y, cfg, palette);
        }
    }
}
