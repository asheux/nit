use ratatui::{buffer::Buffer, layout::Rect};

use nit_gol::Grid;

use super::color::{live_color, trail_color};
use super::geometry::{RenderGeometry, RenderMode};
use super::hud::render_hud_line;
use super::overlay::{cell_bg_halves, draw_bbox_if_any, maybe_draw_debug_overlay, BboxBounds};
use super::palette::GolPalette;
use super::renderer::{
    draw_checker_or_empty, grid_area_below_hud, neighbor_count, GolRenderer, HalfFill,
};
use super::state::{GolHudState, GolRenderConfig, GolRenderState};

const BRAILLE_TOP_ROWS: i32 = 2;

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
        let grid_area = grid_area_below_hud(area);
        if grid_area.width == 0 || grid_area.height == 0 {
            return;
        }

        let geom = RenderGeometry::for_mode(
            RenderMode::Braille,
            grid_area,
            cfg.gol_origin_x,
            cfg.gol_origin_y,
        );
        let use_checker = cfg.grid_minor == Some(1);
        let mut bbox = BboxBounds::empty();

        for ty in 0..grid_area.height {
            for tx in 0..grid_area.width {
                draw_braille_cell(
                    buf,
                    grid,
                    state,
                    &geom,
                    tx,
                    ty,
                    grid_area,
                    cfg,
                    palette,
                    use_checker,
                    &mut bbox,
                );
            }
        }

        draw_bbox_if_any(grid_area, buf, &geom, &bbox, cfg, palette);
        maybe_draw_debug_overlay(grid_area, buf, &geom, cfg, palette);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_braille_cell(
    buf: &mut Buffer,
    grid: &Grid,
    state: &GolRenderState,
    geom: &RenderGeometry,
    tx: u16,
    ty: u16,
    grid_area: Rect,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
    use_checker: bool,
    bbox: &mut BboxBounds,
) {
    let (bg_top, bg_bottom) = cell_bg_halves(geom, tx, ty, cfg, palette);
    let block = sample_braille_block(grid, state, geom, tx, ty, cfg, bbox);

    let cell = buf.get_mut(grid_area.x + tx, grid_area.y + ty);
    if let Some(fill) = HalfFill::from_pair(block.top_alive, block.bottom_alive) {
        cell.set_char(fill.glyph());
        cell.set_fg(live_color(block.max_age, block.max_neighbors, cfg, palette));
        cell.set_bg(fill.bg(bg_top, bg_bottom));
    } else if let Some((fill, decay)) = block.trail_dispatch() {
        cell.set_char(fill.glyph());
        cell.set_fg(trail_color(decay, palette));
        cell.set_bg(fill.bg(bg_top, bg_bottom));
    } else {
        draw_checker_or_empty(cell, bg_top, bg_bottom, use_checker);
    }
}

#[derive(Default)]
struct BrailleBlock {
    top_alive: bool,
    bottom_alive: bool,
    max_age: u8,
    max_neighbors: u8,
    top_decay: u8,
    bottom_decay: u8,
}

impl BrailleBlock {
    fn trail_dispatch(&self) -> Option<(HalfFill, u8)> {
        let top = self.top_decay > 0;
        let bottom = self.bottom_decay > 0;
        let fill = HalfFill::from_pair(top, bottom)?;
        let decay = match fill {
            HalfFill::Both => self.top_decay.max(self.bottom_decay),
            HalfFill::Top => self.top_decay,
            HalfFill::Bottom => self.bottom_decay,
        };
        Some((fill, decay))
    }
}

fn sample_braille_block(
    grid: &Grid,
    state: &GolRenderState,
    geom: &RenderGeometry,
    tx: u16,
    ty: u16,
    cfg: &GolRenderConfig,
    bbox: &mut BboxBounds,
) -> BrailleBlock {
    let (gx0, gy0, gx1, gy1) = geom.term_cell_bounds_in_gol(tx, ty);
    let grid_w = grid.width();
    let grid_h = grid.height();
    let cells = grid.cells();
    let age = &state.age;
    let decay = &state.decay;

    let mut block = BrailleBlock::default();
    for gy in gy0..gy1 {
        let gy_local = gy - geom.gol_origin_y;
        if gy_local < 0 || (gy_local as usize) >= grid_h {
            continue;
        }
        let row_start = (gy_local as usize).saturating_mul(grid_w);
        let is_top = (gy - gy0) < BRAILLE_TOP_ROWS;
        for gx in gx0..gx1 {
            let gx_local = gx - geom.gol_origin_x;
            if gx_local < 0 || (gx_local as usize) >= grid_w {
                continue;
            }
            let idx = row_start + gx_local as usize;
            if cells[idx] != 0 {
                absorb_alive(&mut block, grid, age[idx], gx_local, gy_local, cfg, is_top);
                bbox.include(gx, gy);
            } else if cfg.trails && decay[idx] > 0 {
                absorb_trail(&mut block, decay[idx], is_top);
            }
        }
    }
    block
}

fn absorb_alive(
    block: &mut BrailleBlock,
    grid: &Grid,
    age: u8,
    gx_local: i32,
    gy_local: i32,
    cfg: &GolRenderConfig,
    is_top: bool,
) {
    if is_top {
        block.top_alive = true;
    } else {
        block.bottom_alive = true;
    }
    block.max_age = block.max_age.max(age);
    if cfg.overlay_heat {
        let n = neighbor_count(grid, gx_local as usize, gy_local as usize);
        block.max_neighbors = block.max_neighbors.max(n);
    }
}

fn absorb_trail(block: &mut BrailleBlock, decay: u8, is_top: bool) {
    if is_top {
        block.top_decay = block.top_decay.max(decay);
    } else {
        block.bottom_decay = block.bottom_decay.max(decay);
    }
}

#[cfg(test)]
#[path = "tests/braille.rs"]
mod tests;
