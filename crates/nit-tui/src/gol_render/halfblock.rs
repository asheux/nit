use ratatui::{buffer::Buffer, layout::Rect};

use nit_gol::Grid;

use super::geometry::{RenderGeometry, RenderMode};
use super::palette::GolPalette;
use super::renderer::{
    cell_bg_halves, draw_bbox, live_color, maybe_draw_debug_overlay, neighbor_count,
    render_hud_line, trail_color, GolHudState, GolRenderConfig, GolRenderState, GolRenderer,
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
        let geom = RenderGeometry::for_mode(
            RenderMode::HalfBlock,
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
                let top_y = gy0 - geom.gol_origin_y;
                let bottom_y = top_y + 1;

                let mut top_alive = false;
                let mut bottom_alive = false;
                let mut top_age = 0u8;
                let mut bottom_age = 0u8;
                let mut top_decay = 0u8;
                let mut bottom_decay = 0u8;
                let mut top_neighbors = 0u8;
                let mut bottom_neighbors = 0u8;

                if gx_local >= 0 && (gx_local as usize) < grid_w {
                    let gx = gx_local as usize;
                    if top_y >= 0 && (top_y as usize) < grid_h {
                        let gy = top_y as usize;
                        let idx = gy.saturating_mul(grid_w) + gx;
                        top_alive = cells[idx] != 0;
                        top_age = age[idx];
                        top_decay = decay[idx];
                        if top_alive && cfg.overlay_heat {
                            top_neighbors = neighbor_count(grid, gx, gy);
                        }
                    }
                    if bottom_y >= 0 && (bottom_y as usize) < grid_h {
                        let gy = bottom_y as usize;
                        let idx = gy.saturating_mul(grid_w) + gx;
                        bottom_alive = cells[idx] != 0;
                        bottom_age = age[idx];
                        bottom_decay = decay[idx];
                        if bottom_alive && cfg.overlay_heat {
                            bottom_neighbors = neighbor_count(grid, gx, gy);
                        }
                    }
                }

                if top_alive {
                    any_alive = true;
                    min_x = min_x.min(gx0);
                    min_y = min_y.min(gy0);
                    max_x = max_x.max(gx0);
                    max_y = max_y.max(gy0);
                }
                if bottom_alive {
                    any_alive = true;
                    min_x = min_x.min(gx0);
                    min_y = min_y.min(gy0 + 1);
                    max_x = max_x.max(gx0);
                    max_y = max_y.max(gy0 + 1);
                }

                let any_alive = top_alive || bottom_alive;
                let any_trail = cfg.trails && (top_decay > 0 || bottom_decay > 0);

                let cell = buf.get_mut(x, y);
                if any_alive {
                    let (ch, fg, bg) = if top_alive && bottom_alive {
                        let max_age = top_age.max(bottom_age);
                        let max_neighbors = top_neighbors.max(bottom_neighbors);
                        (
                            '█',
                            live_color(max_age, max_neighbors, cfg, palette),
                            bg_bottom,
                        )
                    } else if top_alive {
                        (
                            '▀',
                            live_color(top_age, top_neighbors, cfg, palette),
                            bg_bottom,
                        )
                    } else {
                        (
                            '▄',
                            live_color(bottom_age, bottom_neighbors, cfg, palette),
                            bg_top,
                        )
                    };
                    cell.set_char(ch);
                    cell.set_fg(fg);
                    cell.set_bg(bg);
                } else if any_trail {
                    if top_decay > 0 && bottom_decay > 0 {
                        let color = trail_color(top_decay.max(bottom_decay), palette);
                        cell.set_char('█');
                        cell.set_fg(color);
                        cell.set_bg(bg_bottom);
                    } else if top_decay > 0 {
                        let color = trail_color(top_decay, palette);
                        cell.set_char('▀');
                        cell.set_fg(color);
                        cell.set_bg(bg_bottom);
                    } else {
                        let color = trail_color(bottom_decay, palette);
                        cell.set_char('▄');
                        cell.set_fg(color);
                        cell.set_bg(bg_top);
                    }
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
#[path = "tests/halfblock.rs"]
mod tests;
