use ratatui::{buffer::Buffer, layout::Rect, style::Color};

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
        let grid_area = grid_area_below_hud(area);
        if grid_area.width == 0 || grid_area.height == 0 {
            return;
        }

        let geom = RenderGeometry::for_mode(
            RenderMode::HalfBlock,
            grid_area,
            cfg.gol_origin_x,
            cfg.gol_origin_y,
        );
        let use_checker = cfg.grid_minor == Some(1);
        let mut bbox = BboxBounds::empty();

        for ty in 0..grid_area.height {
            for tx in 0..grid_area.width {
                draw_halfblock_cell(
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
fn draw_halfblock_cell(
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
    let (gx0, gy0, _gx1, _gy1) = geom.term_cell_bounds_in_gol(tx, ty);
    let sample = sample_halfblock(grid, state, geom, gx0, gy0, cfg.overlay_heat);

    if sample.top.alive {
        bbox.include(gx0, gy0);
    }
    if sample.bottom.alive {
        bbox.include(gx0, gy0 + 1);
    }

    let cell = buf.get_mut(grid_area.x + tx, grid_area.y + ty);
    let Some((glyph, fg, bg)) = decide_paint(&sample, cfg, palette, bg_top, bg_bottom) else {
        draw_checker_or_empty(cell, bg_top, bg_bottom, use_checker);
        return;
    };
    cell.set_char(glyph);
    cell.set_fg(fg);
    cell.set_bg(bg);
}

fn decide_paint(
    sample: &HalfSample,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
    bg_top: Color,
    bg_bottom: Color,
) -> Option<(char, Color, Color)> {
    if let Some(fill) = HalfFill::from_pair(sample.top.alive, sample.bottom.alive) {
        let (age, neighbors) = sample.live_color_inputs(fill);
        let fg = live_color(age, neighbors, cfg, palette);
        return Some((fill.glyph(), fg, fill.bg(bg_top, bg_bottom)));
    }
    let (fill, decay) = sample.trail_dispatch(cfg.trails)?;
    Some((
        fill.glyph(),
        trail_color(decay, palette),
        fill.bg(bg_top, bg_bottom),
    ))
}

#[derive(Default)]
struct HalfCell {
    alive: bool,
    age: u8,
    decay: u8,
    neighbors: u8,
}

struct HalfSample {
    top: HalfCell,
    bottom: HalfCell,
}

impl HalfSample {
    fn live_color_inputs(&self, fill: HalfFill) -> (u8, u8) {
        match fill {
            HalfFill::Both => (
                self.top.age.max(self.bottom.age),
                self.top.neighbors.max(self.bottom.neighbors),
            ),
            HalfFill::Top => (self.top.age, self.top.neighbors),
            HalfFill::Bottom => (self.bottom.age, self.bottom.neighbors),
        }
    }

    fn trail_dispatch(&self, trails_enabled: bool) -> Option<(HalfFill, u8)> {
        if !trails_enabled {
            return None;
        }
        let top = self.top.decay > 0;
        let bottom = self.bottom.decay > 0;
        let fill = HalfFill::from_pair(top, bottom)?;
        let decay = match fill {
            HalfFill::Both => self.top.decay.max(self.bottom.decay),
            HalfFill::Top => self.top.decay,
            HalfFill::Bottom => self.bottom.decay,
        };
        Some((fill, decay))
    }
}

fn sample_halfblock(
    grid: &Grid,
    state: &GolRenderState,
    geom: &RenderGeometry,
    gx0: i32,
    gy0: i32,
    overlay_heat: bool,
) -> HalfSample {
    let gx_local = gx0 - geom.gol_origin_x;
    let top_y = gy0 - geom.gol_origin_y;
    let bottom_y = top_y + 1;
    let grid_w = grid.width();
    let grid_h = grid.height();
    let top = sample_half(grid, state, gx_local, top_y, grid_w, grid_h, overlay_heat);
    let bottom = sample_half(
        grid,
        state,
        gx_local,
        bottom_y,
        grid_w,
        grid_h,
        overlay_heat,
    );
    HalfSample { top, bottom }
}

fn sample_half(
    grid: &Grid,
    state: &GolRenderState,
    gx_local: i32,
    gy_local: i32,
    grid_w: usize,
    grid_h: usize,
    overlay_heat: bool,
) -> HalfCell {
    if gx_local < 0
        || gy_local < 0
        || (gx_local as usize) >= grid_w
        || (gy_local as usize) >= grid_h
    {
        return HalfCell::default();
    }
    let gx = gx_local as usize;
    let gy = gy_local as usize;
    let idx = gy.saturating_mul(grid_w) + gx;
    let alive = grid.cells()[idx] != 0;
    let neighbors = if alive && overlay_heat {
        neighbor_count(grid, gx, gy)
    } else {
        0
    };
    HalfCell {
        alive,
        age: state.age[idx],
        decay: state.decay[idx],
        neighbors,
    }
}

#[cfg(test)]
#[path = "tests/halfblock.rs"]
mod tests;
