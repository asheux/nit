use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use nit_core::{EncodedSeed, SeedPreviewMode};

use super::cache_compute::{BBox, SeedRenderCache};
use super::paint::{bg_style, draw_frame, draw_inset_label, halo_color, mark_blank_glyph};
use super::palette::SeedPalette;
use super::renderer::SeedRenderConfig;

const GRID_TICK_STEP: usize = 4;
const INSET_SIZE: u16 = 16;
const BBOX_OVERLAY_LIMIT: usize = 4;

// Post-pass drawn on top of the mode renderer so overlay glyphs land in the final
// frame regardless of which mode painted the base layer. Order mirrors the HUD's
// visual stacking: grid → bboxes → inset → scanline (last wins on overlap).
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
        draw_grid(area, buf, cfg, palette);
    }
    if cfg.show_bbox {
        draw_bboxes(area, buf, seed, cfg, cache, palette);
    }
    if cfg.show_inset_genome {
        draw_inset(area, buf, cache, palette);
    }
    if cfg.scanline {
        draw_scanline(area, buf, cache, palette);
    }
}

// Mode-dependent pixel-to-terminal scale: bbox coordinates live in the raw grid
// while we paint in terminal cells, so boxes must shrink when a mode packs
// multiple pixels per glyph.
fn scale_for_mode(mode: SeedPreviewMode) -> (usize, usize) {
    match mode {
        SeedPreviewMode::HalfBlock => (1, 2),
        SeedPreviewMode::Braille => (2, 4),
        SeedPreviewMode::Solid | SeedPreviewMode::Tissue | SeedPreviewMode::Heatmap => (1, 1),
    }
}

fn draw_grid(area: Rect, buf: &mut Buffer, cfg: &SeedRenderConfig, palette: &SeedPalette) {
    let (sx, sy) = scale_for_mode(cfg.mode);
    let style = Style::default()
        .fg(palette.grid)
        .add_modifier(Modifier::DIM);
    for y in 0..area.height as usize {
        for x in 0..area.width as usize {
            if !is_grid_tick(x, y, sx, sy) {
                continue;
            }
            mark_blank_glyph(buf, area.x + x as u16, area.y + y as u16, '·', style);
        }
    }
}

// A cell is a grid tick when it lines up with either the horizontal or vertical tick
// stride (converted from pixel space to cell space via the mode's packing factor).
fn is_grid_tick(x: usize, y: usize, sx: usize, sy: usize) -> bool {
    y.saturating_mul(sy) % GRID_TICK_STEP == 0 || x.saturating_mul(sx) % GRID_TICK_STEP == 0
}

fn draw_bboxes(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cfg: &SeedRenderConfig,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    if seed.grid.width() == 0 || seed.grid.height() == 0 {
        return;
    }
    let (sx, sy) = scale_for_mode(cfg.mode);
    let style = Style::default()
        .fg(palette.bbox)
        .add_modifier(Modifier::DIM);
    for bbox in cache.component_bboxes.iter().take(BBOX_OVERLAY_LIMIT) {
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
    draw_frame(
        buf,
        area.x + min_x as u16,
        area.y + min_y as u16,
        area.x + max_x as u16,
        area.y + max_y as u16,
        style,
    );
}

fn draw_inset(area: Rect, buf: &mut Buffer, cache: &SeedRenderCache, palette: &SeedPalette) {
    let Some(bits) = cache.inset_16x16.as_ref() else {
        return;
    };
    if area.width < INSET_SIZE + 2 || area.height < INSET_SIZE + 2 {
        return;
    }
    let start_x = area.x + 1;
    let start_y = area.y + 1;
    paint_inset_cells(buf, bits, start_x, start_y, palette);
    draw_inset_label(buf, start_x, start_y, palette);
    let frame_style = Style::default()
        .fg(palette.grid)
        .add_modifier(Modifier::DIM);
    draw_frame(
        buf,
        start_x - 1,
        start_y - 1,
        start_x + INSET_SIZE,
        start_y + INSET_SIZE,
        frame_style,
    );
}

fn paint_inset_cells(
    buf: &mut Buffer,
    bits: &nit_core::seed::SeedBits,
    start_x: u16,
    start_y: u16,
    palette: &SeedPalette,
) {
    for y in 0..INSET_SIZE as usize {
        for x in 0..INSET_SIZE as usize {
            let fill = if bits.get(x, y) {
                palette.live
            } else {
                palette.bg
            };
            let cell = buf.get_mut(start_x + x as u16, start_y + y as u16);
            cell.set_char(' ');
            cell.set_style(bg_style(fill));
        }
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
        mark_blank_glyph(buf, x, y, ' ', style);
    }
}
