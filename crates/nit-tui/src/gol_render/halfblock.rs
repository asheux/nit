use ratatui::{buffer::Buffer, layout::Rect};

use nit_gol::Grid;

use super::palette::GolPalette;
use super::renderer::{
    draw_bbox, live_color, neighbor_count, render_hud_line, row_bg, trail_color, GolHudState,
    GolRenderConfig, GolRenderState, GolRenderer,
};

#[derive(Default)]
pub struct HalfBlockRenderer;

impl GolRenderer for HalfBlockRenderer {
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
            let top_y = row.saturating_mul(2);
            let bottom_y = top_y.saturating_add(1);
            let top_in = top_y < grid_h;
            let bottom_in = bottom_y < grid_h;
            let top_row_start = if top_in { top_y.saturating_mul(grid_w) } else { 0 };
            let bottom_row_start = if bottom_in { bottom_y.saturating_mul(grid_w) } else { 0 };

            for col in 0..grid_area.width as usize {
                let x = grid_area.x + col as u16;

                let mut top_alive = false;
                let mut bottom_alive = false;
                let mut top_age = 0u8;
                let mut bottom_age = 0u8;
                let mut top_decay = 0u8;
                let mut bottom_decay = 0u8;
                let mut top_neighbors = 0u8;
                let mut bottom_neighbors = 0u8;

                if top_in && col < grid_w {
                    let idx = top_row_start + col;
                    top_alive = cells[idx] != 0;
                    top_age = age[idx];
                    top_decay = decay[idx];
                    if top_alive && cfg.overlay_heat {
                        top_neighbors = neighbor_count(grid, col, top_y);
                    }
                }
                if bottom_in && col < grid_w {
                    let idx = bottom_row_start + col;
                    bottom_alive = cells[idx] != 0;
                    bottom_age = age[idx];
                    bottom_decay = decay[idx];
                    if bottom_alive && cfg.overlay_heat {
                        bottom_neighbors = neighbor_count(grid, col, bottom_y);
                    }
                }

                if top_alive {
                    any_alive = true;
                    min_x = min_x.min(col);
                    min_y = min_y.min(top_y);
                    max_x = max_x.max(col);
                    max_y = max_y.max(top_y);
                }
                if bottom_alive {
                    any_alive = true;
                    min_x = min_x.min(col);
                    min_y = min_y.min(bottom_y);
                    max_x = max_x.max(col);
                    max_y = max_y.max(bottom_y);
                }

                let top_active = top_alive || (cfg.trails && top_decay > 0);
                let bottom_active = bottom_alive || (cfg.trails && bottom_decay > 0);

                let top_color = if top_alive {
                    live_color(top_age, top_neighbors, cfg, palette)
                } else if cfg.trails && top_decay > 0 {
                    trail_color(top_decay, palette)
                } else {
                    bg
                };

                let bottom_color = if bottom_alive {
                    live_color(bottom_age, bottom_neighbors, cfg, palette)
                } else if cfg.trails && bottom_decay > 0 {
                    trail_color(bottom_decay, palette)
                } else {
                    bg
                };

                let (ch, fg) = if top_active && bottom_active {
                    if top_alive || bottom_alive {
                        let max_age = top_age.max(bottom_age);
                        let max_neighbors = top_neighbors.max(bottom_neighbors);
                        ('█', live_color(max_age, max_neighbors, cfg, palette))
                    } else {
                        ('█', trail_color(top_decay.max(bottom_decay), palette))
                    }
                } else if top_active {
                    ('▀', top_color)
                } else if bottom_active {
                    ('▄', bottom_color)
                } else {
                    (' ', bg)
                };

                let cell = buf.get_mut(x, y);
                cell.set_char(ch);
                cell.set_fg(fg);
                cell.set_bg(bg);
            }
        }

        if cfg.overlay_bbox && any_alive {
            let left = min_x;
            let right = max_x;
            let top = min_y / 2;
            let bottom = max_y / 2;
            draw_bbox(grid_area, buf, left, top, right, bottom, cfg, palette);
        }
    }
}
