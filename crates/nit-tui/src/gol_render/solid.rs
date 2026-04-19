use ratatui::{buffer::Buffer, layout::Rect};

use nit_gol::Grid;

use super::color::{live_color, trail_color};
use super::geometry::{RenderGeometry, RenderMode};
use super::hud::render_hud_line;
use super::overlay::{cell_bg_halves, draw_bbox_if_any, maybe_draw_debug_overlay, BboxBounds};
use super::palette::GolPalette;
use super::renderer::{draw_checker_or_empty, grid_area_below_hud, neighbor_count, GolRenderer};
use super::state::{GolHudState, GolRenderConfig, GolRenderState};

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
        let grid_area = grid_area_below_hud(area);
        if grid_area.width == 0 || grid_area.height == 0 {
            return;
        }

        let geom = RenderGeometry::for_mode(
            RenderMode::Solid,
            grid_area,
            cfg.gol_origin_x,
            cfg.gol_origin_y,
        );
        let grid_w = grid.width();
        let grid_h = grid.height();
        let cells = grid.cells();
        let age_buf = &state.age;
        let decay_buf = &state.decay;
        let use_checker = cfg.grid_minor == Some(1);
        let mut bbox = BboxBounds::empty();

        for ty in 0..grid_area.height {
            for tx in 0..grid_area.width {
                let (bg_top, bg_bottom) = cell_bg_halves(&geom, tx, ty, cfg, palette);
                let (gx_abs, gy_abs, _, _) = geom.term_cell_bounds_in_gol(tx, ty);
                let local_x = gx_abs - geom.gol_origin_x;
                let local_y = gy_abs - geom.gol_origin_y;
                let inside = local_x >= 0
                    && local_y >= 0
                    && (local_x as usize) < grid_w
                    && (local_y as usize) < grid_h;
                let cell = buf.get_mut(grid_area.x + tx, grid_area.y + ty);
                if !inside {
                    draw_checker_or_empty(cell, bg_top, bg_bottom, use_checker);
                    continue;
                }
                let gx = local_x as usize;
                let gy = local_y as usize;
                let idx = gy.saturating_mul(grid_w) + gx;
                let alive = cells[idx] != 0;
                if alive {
                    bbox.include(gx_abs, gy_abs);
                    let neighbors = heat_neighbors(grid, gx, gy, cfg.overlay_heat);
                    cell.set_char('▀');
                    cell.set_fg(live_color(age_buf[idx], neighbors, cfg, palette));
                    cell.set_bg(bg_bottom);
                    continue;
                }
                let decay = decay_buf[idx];
                if cfg.trails && decay > 0 {
                    cell.set_char('▀');
                    cell.set_fg(trail_color(decay, palette));
                    cell.set_bg(bg_bottom);
                    continue;
                }
                draw_checker_or_empty(cell, bg_top, bg_bottom, use_checker);
            }
        }

        draw_bbox_if_any(grid_area, buf, &geom, &bbox, cfg, palette);
        maybe_draw_debug_overlay(grid_area, buf, &geom, cfg, palette);
    }
}

fn heat_neighbors(grid: &Grid, gx: usize, gy: usize, overlay_heat: bool) -> u8 {
    if overlay_heat {
        neighbor_count(grid, gx, gy)
    } else {
        0
    }
}

#[cfg(test)]
#[path = "tests/solid.rs"]
mod tests;
