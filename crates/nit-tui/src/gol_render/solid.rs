use ratatui::{buffer::Buffer, layout::Rect};

use nit_gol::Grid;

use super::geometry::{RenderGeometry, RenderMode};
use super::palette::GolPalette;
use super::renderer::{
    cell_bg_halves, draw_bbox, live_color, maybe_draw_debug_overlay, neighbor_count,
    render_hud_line, trail_color, GolHudState, GolRenderConfig, GolRenderState, GolRenderer,
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
        let geom = RenderGeometry::for_mode(
            RenderMode::Solid,
            grid_area,
            cfg.gol_origin_x,
            cfg.gol_origin_y,
        );

        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        let mut any_alive = false;

        let use_checker = cfg.grid_minor == Some(1);
        for row in 0..grid_area.height as usize {
            let ty = row as u16;
            let y = grid_area.y + ty;
            for col in 0..grid_area.width as usize {
                let tx = col as u16;
                let x = grid_area.x + tx;
                let (bg_top, bg_bottom) = cell_bg_halves(&geom, tx, ty, cfg, palette);
                let (gx0, gy0, _gx1, _gy1) = geom.term_cell_bounds_in_gol(tx, ty);
                let gx_local = gx0 - geom.gol_origin_x;
                let gy_local = gy0 - geom.gol_origin_y;
                let mut alive = false;
                let mut age_val = 0u8;
                let mut decay_val = 0u8;
                if gx_local >= 0
                    && gy_local >= 0
                    && (gx_local as usize) < grid_w
                    && (gy_local as usize) < grid_h
                {
                    let gx = gx_local as usize;
                    let gy = gy_local as usize;
                    let idx = gy.saturating_mul(grid_w) + gx;
                    alive = cells[idx] != 0;
                    age_val = age[idx];
                    decay_val = decay[idx];
                }

                if alive {
                    any_alive = true;
                    min_x = min_x.min(gx0);
                    min_y = min_y.min(gy0);
                    max_x = max_x.max(gx0);
                    max_y = max_y.max(gy0);
                }

                let cell = buf.get_mut(x, y);
                if alive {
                    let neighbors = if cfg.overlay_heat {
                        neighbor_count(grid, gx_local as usize, gy_local as usize)
                    } else {
                        0
                    };
                    let color = live_color(age_val, neighbors, cfg, palette);
                    cell.set_char('▀');
                    cell.set_fg(color);
                    cell.set_bg(bg_bottom);
                } else if cfg.trails && decay_val > 0 {
                    let color = trail_color(decay_val, palette);
                    cell.set_char('▀');
                    cell.set_fg(color);
                    cell.set_bg(bg_bottom);
                } else if use_checker {
                    cell.set_char('▀');
                    cell.set_fg(bg_top);
                    cell.set_bg(bg_bottom);
                } else {
                    cell.set_char(' ');
                    cell.set_fg(bg_bottom);
                    cell.set_bg(bg_bottom);
                }
            }
        }

        if cfg.overlay_bbox && any_alive {
            if let (Some((left, top)), Some((right, bottom))) = (
                geom.gol_to_term(min_x, min_y),
                geom.gol_to_term(max_x, max_y),
            ) {
                draw_bbox(
                    grid_area,
                    buf,
                    left as usize,
                    top as usize,
                    right as usize,
                    bottom as usize,
                    cfg,
                    palette,
                );
            }
        }

        maybe_draw_debug_overlay(grid_area, buf, &geom, cfg, palette);
    }
}

#[cfg(test)]
#[path = "tests/solid.rs"]
mod tests;
