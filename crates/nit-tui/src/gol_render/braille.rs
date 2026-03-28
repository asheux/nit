use ratatui::{buffer::Buffer, layout::Rect};

use nit_gol::Grid;

use super::geometry::{RenderGeometry, RenderMode};
use super::palette::GolPalette;
use super::renderer::{
    cell_bg_halves, draw_bbox, live_color, maybe_draw_debug_overlay, neighbor_count,
    render_hud_line, trail_color, GolHudState, GolRenderConfig, GolRenderState, GolRenderer,
};

#[derive(Default)]
pub struct BrailleRenderer;

impl GolRenderer for BrailleRenderer {
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
            RenderMode::Braille,
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
                let (gx0, gy0, gx1, gy1) = geom.term_cell_bounds_in_gol(tx, ty);
                let mut max_age = 0u8;
                let mut max_decay = 0u8;
                let mut max_decay_top = 0u8;
                let mut max_decay_bottom = 0u8;
                let mut max_neighbors = 0u8;
                let mut any_alive_block = false;
                let mut any_trail_block = false;
                let mut top_trail = false;
                let mut bottom_trail = false;
                let mut top_alive = false;
                let mut bottom_alive = false;

                let mut gy = gy0;
                while gy < gy1 {
                    let gy_local = gy - geom.gol_origin_y;
                    if gy_local >= 0 && (gy_local as usize) < grid_h {
                        let row_start = (gy_local as usize).saturating_mul(grid_w);
                        let mut gx = gx0;
                        while gx < gx1 {
                            let gx_local = gx - geom.gol_origin_x;
                            if gx_local >= 0 && (gx_local as usize) < grid_w {
                                let idx = row_start + gx_local as usize;
                                let alive = cells[idx] != 0;
                                let decay_val = decay[idx];
                                if alive {
                                    any_alive_block = true;
                                    let dy = gy - gy0;
                                    if dy < 2 {
                                        top_alive = true;
                                    } else {
                                        bottom_alive = true;
                                    }
                                    max_age = max_age.max(age[idx]);
                                    if cfg.overlay_heat {
                                        max_neighbors = max_neighbors.max(neighbor_count(
                                            grid,
                                            gx_local as usize,
                                            gy_local as usize,
                                        ));
                                    }
                                    any_alive = true;
                                    min_x = min_x.min(gx);
                                    min_y = min_y.min(gy);
                                    max_x = max_x.max(gx);
                                    max_y = max_y.max(gy);
                                } else if cfg.trails && decay_val > 0 {
                                    any_trail_block = true;
                                    max_decay = max_decay.max(decay_val);
                                    let dy = gy - gy0;
                                    if dy < 2 {
                                        top_trail = true;
                                        max_decay_top = max_decay_top.max(decay_val);
                                    } else {
                                        bottom_trail = true;
                                        max_decay_bottom = max_decay_bottom.max(decay_val);
                                    }
                                }
                            }
                            gx += 1;
                        }
                    }
                    gy += 1;
                }

                let cell = buf.get_mut(x, y);
                if any_alive_block {
                    let fg = live_color(max_age, max_neighbors, cfg, palette);
                    let (ch, bg) = if top_alive && bottom_alive {
                        ('█', bg_bottom)
                    } else if top_alive {
                        ('▀', bg_bottom)
                    } else {
                        ('▄', bg_top)
                    };
                    cell.set_char(ch);
                    cell.set_fg(fg);
                    cell.set_bg(bg);
                } else if any_trail_block {
                    if top_trail && bottom_trail {
                        let color = trail_color(max_decay, palette);
                        cell.set_char('█');
                        cell.set_fg(color);
                        cell.set_bg(bg_bottom);
                    } else if top_trail {
                        let color = trail_color(max_decay_top, palette);
                        cell.set_char('▀');
                        cell.set_fg(color);
                        cell.set_bg(bg_bottom);
                    } else {
                        let color = trail_color(max_decay_bottom, palette);
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
#[path = "tests/braille.rs"]
mod tests;
