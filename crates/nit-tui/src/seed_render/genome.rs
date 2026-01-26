use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use nit_core::{EncodedSeed, SeedEncoderId};

use super::palette::SeedPalette;
use super::renderer::SeedRenderCache;

const SHADE_CHARS: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

pub fn render_genome(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    match seed.encoder_id {
        SeedEncoderId::Lifehash16 => render_lifehash16(area, buf, seed, cache, palette),
        SeedEncoderId::HilbertBits => render_hilbert_bits(area, buf, seed, cache, palette),
        SeedEncoderId::AsciiBytes => render_ascii_bytes(area, buf, seed, cache, palette),
    }
}

fn render_lifehash16(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    _cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    let w = seed.base_bits.width().max(1);
    let h = seed.base_bits.height().max(1);
    let max_w = area.width as usize;
    let max_h = area.height as usize;
    let out_w = max_w.max(1);
    let out_h = max_h.max(1);

    for y in 0..out_h {
        let sy = y.saturating_mul(h) / out_h;
        for x in 0..out_w {
            let sx = x.saturating_mul(w) / out_w;
            let alive = seed.base_bits.get(sx, sy);
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            cell.set_char(' ');
            cell.set_style(Style::default().bg(if alive {
                palette.accent_2
            } else {
                palette.bg
            }));
        }
    }

    draw_symmetry(area, buf, seed, palette);
}

fn draw_symmetry(area: Rect, buf: &mut Buffer, seed: &EncodedSeed, palette: &SeedPalette) {
    let style = Style::default()
        .fg(palette.accent_2)
        .add_modifier(Modifier::DIM);
    let mid_x = area.x + area.width / 2;
    let mid_y = area.y + area.height / 2;
    match seed.params.symmetry {
        nit_core::SeedSymmetry::MirrorX => {
            for y in area.y..area.y.saturating_add(area.height) {
                let cell = buf.get_mut(mid_x, y);
                if cell.symbol() == " " {
                    cell.set_char('·');
                    cell.set_style(style);
                }
            }
        }
        nit_core::SeedSymmetry::MirrorY => {
            for x in area.x..area.x.saturating_add(area.width) {
                let cell = buf.get_mut(x, mid_y);
                if cell.symbol() == " " {
                    cell.set_char('·');
                    cell.set_style(style);
                }
            }
        }
        nit_core::SeedSymmetry::Rotate180 => {
            for y in area.y..area.y.saturating_add(area.height) {
                let cell = buf.get_mut(mid_x, y);
                if cell.symbol() == " " {
                    cell.set_char('·');
                    cell.set_style(style);
                }
            }
            for x in area.x..area.x.saturating_add(area.width) {
                let cell = buf.get_mut(x, mid_y);
                if cell.symbol() == " " {
                    cell.set_char('·');
                    cell.set_style(style);
                }
            }
        }
        nit_core::SeedSymmetry::None => {}
    }
}

fn render_hilbert_bits(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    _cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    let w = seed.base_bits.width().max(1);
    let h = seed.base_bits.height().max(1);
    let total = w.saturating_mul(h).max(1);
    let cols = area.width as usize;
    let rows = area.height as usize;
    if cols == 0 || rows == 0 {
        return;
    }
    for x in 0..cols {
        let start = x.saturating_mul(total) / cols.max(1);
        let end = (x + 1).saturating_mul(total) / cols.max(1);
        let mut count = 0usize;
        let mut i = start;
        while i < end {
            let sx = i % w;
            let sy = i / w;
            if seed.base_bits.get(sx, sy) {
                count += 1;
            }
            i += 1;
        }
        let span = (end - start).max(1);
        let height = (count * rows) / span;
        let sep = start % 32 == 0;
        for y in 0..rows {
            let draw = rows.saturating_sub(y + 1) < height;
            let ch = if draw { '█' } else if sep { '┆' } else { ' ' };
            let fg = if draw { palette.accent_2 } else { palette.grid };
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            cell.set_char(ch);
            cell.set_style(Style::default().fg(fg).bg(palette.bg));
        }
    }
}

fn render_ascii_bytes(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    _cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    let w = seed.base_values.width().max(1);
    let h = seed.base_values.height().max(1);
    let cols = area.width as usize;
    let rows = area.height as usize;
    if cols == 0 || rows == 0 {
        return;
    }
    for y in 0..rows {
        let sy = y.saturating_mul(h) / rows;
        for x in 0..cols {
            let sx = x.saturating_mul(w) / cols;
            let value = seed.base_values.get(sx, sy) as usize;
            let idx = value.saturating_mul(SHADE_CHARS.len() - 1) / 255;
            let ch = SHADE_CHARS[idx];
            let fg = if value > 200 {
                palette.accent_2
            } else if value > 120 {
                palette.live_dim
            } else {
                palette.hud_dim
            };
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            cell.set_char(ch);
            cell.set_style(Style::default().fg(fg).bg(palette.bg));
        }
    }
}
