use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use nit_core::{EncodedSeed, SeedPreviewMode};

use super::palette::SeedPalette;
use super::renderer::{halo_color, BBox, SeedRenderCache, SeedRenderConfig};

pub fn render_overlays(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if cfg.show_grid {
        draw_grid(area, buf, seed, cfg, palette);
    }
    if cfg.show_bbox {
        draw_bboxes(area, buf, seed, cfg, cache, palette);
    }
    if cfg.show_inset_genome {
        draw_inset(area, buf, seed, cfg, cache, palette);
    }
    if cfg.scanline {
        draw_scanline(area, buf, cache, palette);
    }
}

fn scale_for_mode(mode: SeedPreviewMode) -> (usize, usize) {
    match mode {
        SeedPreviewMode::HalfBlock => (1, 2),
        SeedPreviewMode::Braille => (2, 4),
        SeedPreviewMode::Solid | SeedPreviewMode::Tissue | SeedPreviewMode::Heatmap => (1, 1),
    }
}

fn draw_grid(
    area: Rect,
    buf: &mut Buffer,
    _seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    palette: &SeedPalette,
) {
    let (sx, sy) = scale_for_mode(cfg.mode);
    let step = 4usize;
    let w = area.width as usize;
    let h = area.height as usize;
    let style = Style::default()
        .fg(palette.grid)
        .add_modifier(Modifier::DIM);
    for y in 0..h {
        let gy = y.saturating_mul(sy);
        let y_line = gy % step == 0;
        for x in 0..w {
            let gx = x.saturating_mul(sx);
            if !y_line && gx % step != 0 {
                continue;
            }
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            if cell.symbol() == " " {
                cell.set_char('·');
                cell.set_style(style);
            }
        }
    }
}

fn draw_bboxes(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    let (sx, sy) = scale_for_mode(cfg.mode);
    let w = seed.grid.width();
    let h = seed.grid.height();
    if w == 0 || h == 0 {
        return;
    }
    let style = Style::default()
        .fg(palette.bbox)
        .add_modifier(Modifier::DIM);
    for bbox in cache.component_bboxes.iter().take(4) {
        draw_bbox(area, buf, bbox, sx, sy, style);
    }
}

fn draw_bbox(area: Rect, buf: &mut Buffer, bbox: &BBox, sx: usize, sy: usize, style: Style) {
    let min_x = bbox.min_x / sx;
    let max_x = bbox.max_x / sx;
    let min_y = bbox.min_y / sy;
    let max_y = bbox.max_y / sy;
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    let left = area.x + min_x as u16;
    let right = area.x + max_x as u16;
    let top = area.y + min_y as u16;
    let bottom = area.y + max_y as u16;
    for x in left..=right {
        let cell_top = buf.get_mut(x, top);
        cell_top.set_char('─');
        cell_top.set_style(style);
        let cell_bottom = buf.get_mut(x, bottom);
        cell_bottom.set_char('─');
        cell_bottom.set_style(style);
    }
    for y in top..=bottom {
        let cell_left = buf.get_mut(left, y);
        cell_left.set_char('│');
        cell_left.set_style(style);
        let cell_right = buf.get_mut(right, y);
        cell_right.set_char('│');
        cell_right.set_style(style);
    }
    buf.get_mut(left, top).set_char('┌');
    buf.get_mut(right, top).set_char('┐');
    buf.get_mut(left, bottom).set_char('└');
    buf.get_mut(right, bottom).set_char('┘');
}

fn draw_inset(
    area: Rect,
    buf: &mut Buffer,
    _seed: &EncodedSeed,
    _cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    let Some(bits) = cache.inset_16x16.as_ref() else {
        return;
    };
    let inset_w = 16u16;
    let inset_h = 16u16;
    if area.width < inset_w + 2 || area.height < inset_h + 2 {
        return;
    }
    let start_x = area.x + 1;
    let start_y = area.y + 1;
    for y in 0..16usize {
        for x in 0..16usize {
            let alive = bits.get(x, y);
            let cell = buf.get_mut(start_x + x as u16, start_y + y as u16);
            cell.set_char(' ');
            cell.set_style(Style::default().bg(if alive { palette.live } else { palette.bg }));
        }
    }
    draw_inset_label(start_x, start_y, palette, buf);
    // tiny frame
    let style = Style::default()
        .fg(palette.grid)
        .add_modifier(Modifier::DIM);
    let right = start_x + inset_w;
    let bottom = start_y + inset_h;
    for x in start_x..=right {
        buf.get_mut(x, start_y - 1).set_char('─');
        buf.get_mut(x, start_y - 1).set_style(style);
        buf.get_mut(x, bottom).set_char('─');
        buf.get_mut(x, bottom).set_style(style);
    }
    for y in start_y..=bottom {
        buf.get_mut(start_x - 1, y).set_char('│');
        buf.get_mut(start_x - 1, y).set_style(style);
        buf.get_mut(right, y).set_char('│');
        buf.get_mut(right, y).set_style(style);
    }
    buf.get_mut(start_x - 1, start_y - 1).set_char('┌');
    buf.get_mut(right, start_y - 1).set_char('┐');
    buf.get_mut(start_x - 1, bottom).set_char('└');
    buf.get_mut(right, bottom).set_char('┘');
}

fn draw_inset_label(start_x: u16, start_y: u16, palette: &SeedPalette, buf: &mut Buffer) {
    let label = "INSET";
    let style = Style::default()
        .fg(palette.hud_dim)
        .add_modifier(Modifier::DIM);
    let y = start_y.saturating_sub(1);
    let mut x = start_x;
    for ch in label.chars() {
        let cell = buf.get_mut(x, y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
}

fn draw_scanline(area: Rect, buf: &mut Buffer, cache: &SeedRenderCache, palette: &SeedPalette) {
    if area.height == 0 {
        return;
    }
    let y = (cache.scanline_phase % area.height).saturating_add(area.y);
    let style = Style::default()
        .bg(halo_color(1, palette))
        .fg(palette.hud_dim)
        .add_modifier(Modifier::DIM);
    for x in area.x..area.x.saturating_add(area.width) {
        let cell = buf.get_mut(x, y);
        if cell.symbol() == " " {
            cell.set_char(' ');
            cell.set_style(style);
        }
    }
}
