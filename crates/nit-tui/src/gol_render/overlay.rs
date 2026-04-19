use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::color::row_bg;
use super::geometry::RenderGeometry;
use super::hud::{write_str, write_u32};
use super::palette::{darken, GolPalette};
use super::state::GolRenderConfig;

// Grid overlay darkens the cell bg at minor/major gridlines; strong for
// intersections or major-only, soft for single minor lines.
const GRID_DARKEN_STRONG: f32 = 0.82;
const GRID_DARKEN_SOFT: f32 = 0.9;
const DEBUG_CROSSHAIRS: [(i32, i32); 2] = [(0, 0), (16, 16)];
const DEBUG_AXIS_STOPS: [i32; 3] = [0, 16, 32];

fn spans_gridline(start: i32, count: u16, spacing: u16) -> bool {
    if spacing == 0 {
        return false;
    }
    let spacing = spacing as i32;
    let mut offset = 0i32;
    while offset < count as i32 {
        let k = start + offset;
        if k % spacing == 0 {
            return true;
        }
        offset += 1;
    }
    false
}

pub(crate) fn gridline_flags(
    geom: &RenderGeometry,
    tx: u16,
    ty: u16,
    spacing: u16,
) -> (bool, bool) {
    if spacing == 0 {
        return (false, false);
    }
    let (gx0, gy0, _gx1, _gy1) = geom.term_cell_bounds_in_gol(tx, ty);
    let v = spans_gridline(gx0, geom.cell_per_term_x, spacing);
    let h = spans_gridline(gy0, geom.cell_per_term_y, spacing);
    (v, h)
}

pub(crate) fn cell_bg(
    geom: &RenderGeometry,
    tx: u16,
    ty: u16,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) -> Color {
    let base = row_bg(ty as usize, cfg, palette);
    if cfg.grid_minor == Some(1) {
        let parity = ((tx as u32 + ty as u32) & 1) == 0;
        let alt = if base == palette.bg {
            palette.scanline
        } else {
            palette.bg
        };
        let mut bg = if parity { base } else { alt };
        if let Some(spacing) = cfg.grid_major {
            let (major_v, major_h) = gridline_flags(geom, tx, ty, spacing);
            if major_v || major_h {
                bg = alt;
            }
        }
        return bg;
    }
    let minor = cfg.grid_minor;
    let major = cfg.grid_major;
    if minor.is_none() && major.is_none() {
        return base;
    }
    let (minor_v, minor_h) = minor
        .map(|spacing| gridline_flags(geom, tx, ty, spacing))
        .unwrap_or((false, false));
    let (major_v, major_h) = major
        .map(|spacing| gridline_flags(geom, tx, ty, spacing))
        .unwrap_or((false, false));
    let v = minor_v || major_v;
    let h = minor_h || major_h;
    if !v && !h {
        return base;
    }
    let strong = major_v || major_h || (v && h);
    let factor = if strong {
        GRID_DARKEN_STRONG
    } else {
        GRID_DARKEN_SOFT
    };
    darken(base, factor)
}

pub(crate) fn cell_bg_halves(
    geom: &RenderGeometry,
    tx: u16,
    ty: u16,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) -> (Color, Color) {
    if cfg.grid_minor != Some(1) {
        let bg = cell_bg(geom, tx, ty, cfg, palette);
        return (bg, bg);
    }

    let base = row_bg(ty as usize, cfg, palette);
    let alt = if base == palette.bg {
        palette.scanline
    } else {
        palette.bg
    };
    let (gx0, gy0, _gx1, _gy1) = geom.term_cell_bounds_in_gol(tx, ty);
    let cell_x = geom.cell_per_term_x.max(1);
    let half_y = (geom.cell_per_term_y / 2).max(1);
    let px = (gx0).div_euclid(cell_x as i32);
    let py_top = (gy0).div_euclid(half_y as i32);
    let py_bottom = (gy0 + half_y as i32).div_euclid(half_y as i32);

    let mut top = if (px + py_top).rem_euclid(2) == 0 {
        base
    } else {
        alt
    };
    let mut bottom = if (px + py_bottom).rem_euclid(2) == 0 {
        base
    } else {
        alt
    };

    if let Some(spacing) = cfg.grid_major {
        let major_v = spans_gridline(gx0, cell_x, spacing);
        let major_h_top = spans_gridline(gy0, half_y, spacing);
        let major_h_bottom = spans_gridline(gy0 + half_y as i32, half_y, spacing);
        if major_v || major_h_top {
            top = alt;
        }
        if major_v || major_h_bottom {
            bottom = alt;
        }
    }

    (top, bottom)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BboxBounds {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub any: bool,
}

impl BboxBounds {
    pub fn empty() -> Self {
        Self {
            min_x: i32::MAX,
            min_y: i32::MAX,
            max_x: i32::MIN,
            max_y: i32::MIN,
            any: false,
        }
    }

    pub fn include(&mut self, gx: i32, gy: i32) {
        self.min_x = self.min_x.min(gx);
        self.min_y = self.min_y.min(gy);
        self.max_x = self.max_x.max(gx);
        self.max_y = self.max_y.max(gy);
        self.any = true;
    }
}

pub(crate) fn draw_bbox_if_any(
    grid_area: Rect,
    buf: &mut Buffer,
    geom: &RenderGeometry,
    bbox: &BboxBounds,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) {
    if !cfg.overlay_bbox || !bbox.any {
        return;
    }
    let Some((left, top)) = geom.gol_to_term(bbox.min_x, bbox.min_y) else {
        return;
    };
    let Some((right, bottom)) = geom.gol_to_term(bbox.max_x, bbox.max_y) else {
        return;
    };
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

#[allow(clippy::too_many_arguments)]
fn draw_bbox(
    grid_area: Rect,
    buf: &mut Buffer,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) {
    if grid_area.width == 0 || grid_area.height == 0 {
        return;
    }
    if right < left || bottom < top {
        return;
    }
    let max_x = (grid_area.width as usize).saturating_sub(1);
    let max_y = (grid_area.height as usize).saturating_sub(1);
    let left = left.min(max_x) as u16;
    let right = right.min(max_x) as u16;
    let top = top.min(max_y) as u16;
    let bottom = bottom.min(max_y) as u16;
    if left > right || top > bottom {
        return;
    }

    let style = Style::default()
        .fg(palette.bbox)
        .add_modifier(Modifier::DIM);

    for x in left..=right {
        let y = top;
        let bg = row_bg(y as usize, cfg, palette);
        let cell = buf.get_mut(grid_area.x + x, grid_area.y + y);
        cell.set_char(if x == left {
            '┌'
        } else if x == right {
            '┐'
        } else {
            '─'
        });
        cell.set_style(style);
        cell.set_bg(bg);
    }

    if bottom != top {
        for x in left..=right {
            let y = bottom;
            let bg = row_bg(y as usize, cfg, palette);
            let cell = buf.get_mut(grid_area.x + x, grid_area.y + y);
            cell.set_char(if x == left {
                '└'
            } else if x == right {
                '┘'
            } else {
                '─'
            });
            cell.set_style(style);
            cell.set_bg(bg);
        }
    }

    for y in (top + 1)..bottom {
        let bg = row_bg(y as usize, cfg, palette);
        let cell_left = buf.get_mut(grid_area.x + left, grid_area.y + y);
        cell_left.set_char('│');
        cell_left.set_style(style);
        cell_left.set_bg(bg);
        if right != left {
            let cell_right = buf.get_mut(grid_area.x + right, grid_area.y + y);
            cell_right.set_char('│');
            cell_right.set_style(style);
            cell_right.set_bg(bg);
        }
    }
}

pub(crate) fn maybe_draw_debug_overlay(
    grid_area: Rect,
    buf: &mut Buffer,
    geom: &RenderGeometry,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) {
    if !cfg.debug_overlay || grid_area.width == 0 || grid_area.height == 0 {
        return;
    }
    draw_debug_crosshairs(grid_area, buf, geom, palette);
    draw_debug_axis_labels(grid_area, buf, geom, palette);
    draw_grid_intersection_marks(grid_area, buf, geom, cfg, palette);
}

fn draw_debug_crosshairs(
    grid_area: Rect,
    buf: &mut Buffer,
    geom: &RenderGeometry,
    palette: &GolPalette,
) {
    let style = Style::default()
        .fg(palette.hud_text)
        .add_modifier(Modifier::DIM);
    let bound_x = grid_area.x.saturating_add(grid_area.width);
    let bound_y = grid_area.y.saturating_add(grid_area.height);
    for (gx, gy) in DEBUG_CROSSHAIRS {
        let Some((tx, ty)) = geom.gol_to_term(gx, gy) else {
            continue;
        };
        let x = grid_area.x.saturating_add(tx);
        let y = grid_area.y.saturating_add(ty);
        if x >= bound_x || y >= bound_y {
            continue;
        }
        let cell = buf.get_mut(x, y);
        let keep_bg = cell.bg;
        cell.set_char('+');
        cell.set_style(style);
        cell.set_bg(keep_bg);
    }
}

fn draw_debug_axis_labels(
    grid_area: Rect,
    buf: &mut Buffer,
    geom: &RenderGeometry,
    palette: &GolPalette,
) {
    let max_x = grid_area.x.saturating_add(grid_area.width);
    let style = Style::default()
        .fg(palette.hud_dim)
        .add_modifier(Modifier::DIM);
    let y0 = grid_area
        .y
        .saturating_add(grid_area.height.saturating_sub(2));
    let y1 = grid_area
        .y
        .saturating_add(grid_area.height.saturating_sub(1));
    let mut x = write_str(buf, grid_area.x, y0, max_x, style, "GX:");
    for gx in DEBUG_AXIS_STOPS {
        x = write_axis_entry(buf, x, y0, max_x, style, gx, |g| {
            geom.gol_to_term(g, geom.gol_origin_y).map(|(tx, _)| tx)
        });
    }
    let mut x = write_str(buf, grid_area.x, y1, max_x, style, "GY:");
    for gy in DEBUG_AXIS_STOPS {
        x = write_axis_entry(buf, x, y1, max_x, style, gy, |g| {
            geom.gol_to_term(geom.gol_origin_x, g).map(|(_, ty)| ty)
        });
    }
}

fn write_axis_entry(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    value: i32,
    project: impl FnOnce(i32) -> Option<u16>,
) -> u16 {
    let mut x = write_str(buf, x, y, max_x, style, " ");
    x = write_u32(buf, x, y, max_x, style, value as u32, 0);
    x = write_str(buf, x, y, max_x, style, "->");
    match project(value) {
        Some(t) => write_u32(buf, x, y, max_x, style, t as u32, 0),
        None => write_str(buf, x, y, max_x, style, "--"),
    }
}

fn draw_grid_intersection_marks(
    grid_area: Rect,
    buf: &mut Buffer,
    geom: &RenderGeometry,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) {
    if cfg.grid_minor.is_none() && cfg.grid_major.is_none() {
        return;
    }
    let mark_style = Style::default()
        .fg(palette.hud_text)
        .add_modifier(Modifier::DIM);
    for ty in 0..grid_area.height {
        for tx in 0..grid_area.width {
            let (minor_v, minor_h) = cfg
                .grid_minor
                .map(|spacing| gridline_flags(geom, tx, ty, spacing))
                .unwrap_or((false, false));
            let (major_v, major_h) = cfg
                .grid_major
                .map(|spacing| gridline_flags(geom, tx, ty, spacing))
                .unwrap_or((false, false));
            let v = minor_v || major_v;
            let h = minor_h || major_h;
            if !(v && h) {
                continue;
            }
            let x = grid_area.x.saturating_add(tx);
            let y = grid_area.y.saturating_add(ty);
            let cell = buf.get_mut(x, y);
            let bg = cell_bg(geom, tx, ty, cfg, palette);
            if cell.symbol() == " " && cell.bg == bg {
                let keep_bg = cell.bg;
                cell.set_char('.');
                cell.set_style(mark_style);
                cell.set_bg(keep_bg);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/renderer.rs"]
mod tests;
