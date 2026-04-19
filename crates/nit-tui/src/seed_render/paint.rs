use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};

use super::palette::SeedPalette;
use super::renderer::SeedRenderCache;

pub(super) fn halo_color(intensity: u8, palette: &SeedPalette) -> Color {
    if intensity >= 3 {
        palette.halo_2
    } else {
        palette.halo_1
    }
}

pub(super) fn halo_bg_at(
    x: usize,
    y: usize,
    grid_w: usize,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) -> Color {
    let Some(mask) = cache.halo_mask.as_ref() else {
        return palette.bg;
    };
    match mask.get(y * grid_w + x).copied() {
        Some(v) if v > 0 => halo_color(v, palette),
        _ => palette.bg,
    }
}

pub(super) fn bg_style(bg: Color) -> Style {
    Style::default().bg(bg)
}

pub(super) fn write_glyph(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style) {
    let cell = buf.get_mut(x, y);
    cell.set_char(ch);
    cell.set_style(style);
}

pub(super) fn mark_blank_glyph(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style) {
    let cell = buf.get_mut(x, y);
    if cell.symbol() == " " {
        cell.set_char(ch);
        cell.set_style(style);
    }
}

// Rectangular frame with box-drawing edges and corner glyphs. Coordinates are
// inclusive; callers must ensure `x1 >= x0` and `y1 >= y0`.
pub(super) fn draw_frame(buf: &mut Buffer, x0: u16, y0: u16, x1: u16, y1: u16, style: Style) {
    for x in x0..=x1 {
        write_glyph(buf, x, y0, '─', style);
        write_glyph(buf, x, y1, '─', style);
    }
    for y in y0..=y1 {
        write_glyph(buf, x0, y, '│', style);
        write_glyph(buf, x1, y, '│', style);
    }
    buf.get_mut(x0, y0).set_char('┌');
    buf.get_mut(x1, y0).set_char('┐');
    buf.get_mut(x0, y1).set_char('└');
    buf.get_mut(x1, y1).set_char('┘');
}

pub(super) fn draw_inset_label(
    buf: &mut Buffer,
    start_x: u16,
    start_y: u16,
    palette: &SeedPalette,
) {
    let style = Style::default()
        .fg(palette.hud_dim)
        .add_modifier(Modifier::DIM);
    let y = start_y.saturating_sub(1);
    let mut x = start_x;
    for ch in "INSET".chars() {
        write_glyph(buf, x, y, ch, style);
        x = x.saturating_add(1);
    }
}
