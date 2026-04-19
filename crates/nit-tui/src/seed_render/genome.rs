use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use nit_core::seed::SeedBits;
use nit_core::{EncodedSeed, SeedEncoderId, SeedSymmetry};

use super::cache_compute::SeedRenderCache;
use super::paint::{bg_style, draw_inset_label, mark_blank_glyph, write_glyph};
use super::palette::SeedPalette;

const INSET_SIZE: u16 = 16;
const VALUE_TIER_HIGH: u16 = 200;
const VALUE_TIER_MID: u16 = 120;

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
        SeedEncoderId::Lifehash16 => render_lifehash16(area, buf, seed, palette),
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => {
            render_hilbert_bits(area, buf, seed, cache, palette)
        }
        SeedEncoderId::AsciiBytes
        | SeedEncoderId::TokenSpectrum
        | SeedEncoderId::AstStructure
        | SeedEncoderId::ComplexityField => render_ascii_bytes(area, buf, seed, palette),
    }
}

fn render_lifehash16(area: Rect, buf: &mut Buffer, seed: &EncodedSeed, palette: &SeedPalette) {
    render_bitgrid_bits(area, buf, &seed.base_bits_raw, palette.accent_2, palette.bg);
    draw_symmetry(area, buf, seed.params.symmetry, palette);
    draw_lifehash_inset(area, buf, seed, palette);
}

fn render_hilbert_bits(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    let Some(stream) = cache.hilbert_stream.as_ref() else {
        render_bitgrid_bits(area, buf, &seed.base_bits, palette.accent_2, palette.bg);
        return;
    };
    let cols = area.width as usize;
    let rows = area.height as usize;
    if cols == 0 || rows == 0 {
        return;
    }
    let total = stream.len().max(1);
    let stride = total.div_ceil(cols).max(1);
    let sep_style_for = |bg: Color| {
        Style::default()
            .fg(palette.grid)
            .bg(bg)
            .add_modifier(Modifier::DIM)
    };
    for col in 0..cols {
        let idx = col.saturating_mul(stride);
        let lit = idx < total && stream[idx] != 0;
        let bg = if lit { palette.accent_2 } else { palette.bg };
        let sep = hilbert_sep_for(idx);
        let (glyph, style) = match sep {
            Some(ch) => (ch, sep_style_for(bg)),
            None => (' ', bg_style(bg)),
        };
        let col_x = area.x + col as u16;
        for row in 0..rows {
            write_glyph(buf, col_x, area.y + row as u16, glyph, style);
        }
    }
    draw_hilbert_inset(area, buf, cache, palette);
}

fn hilbert_sep_for(idx: usize) -> Option<char> {
    if idx % 64 == 0 {
        Some('┆')
    } else if idx % 16 == 0 {
        Some('┊')
    } else if idx % 8 == 0 {
        Some('│')
    } else {
        None
    }
}

fn render_ascii_bytes(area: Rect, buf: &mut Buffer, seed: &EncodedSeed, palette: &SeedPalette) {
    let grid_w = seed.base_values.width().max(1);
    let grid_h = seed.base_values.height().max(1);
    let max_cols = area.width as usize;
    let max_rows = area.height as usize;
    if max_cols == 0 || max_rows == 0 {
        return;
    }
    let (digits, gap, cols) = ascii_layout(max_cols, grid_w);
    if cols == 0 {
        return;
    }
    // Keep the byte grid low-chroma: printable bytes are marked with a neutral gray, while the
    // full accent color remains reserved for selection/crosshair.
    let printable_fg = gray_from(palette.hud_text, 60);
    let rows = max_rows.max(1);
    let stride = digits + gap;
    for row in 0..rows {
        let sy = row.saturating_mul(grid_h) / rows;
        let cy = area.y + row as u16;
        for col in 0..cols {
            let sx = col.saturating_mul(grid_w) / cols;
            let cell = ByteCellCtx {
                start_x: area.x + (col * stride) as u16,
                cy,
                sx,
                x: col,
                value: seed.base_values.get(sx, sy) as u16,
                digits,
                gap,
                printable_fg,
            };
            draw_byte_cell(buf, cell, palette);
        }
    }
}

struct ByteCellCtx {
    start_x: u16,
    cy: u16,
    sx: usize,
    x: usize,
    value: u16,
    digits: usize,
    gap: usize,
    printable_fg: Color,
}

fn draw_byte_cell(buf: &mut Buffer, ctx: ByteCellCtx, palette: &SeedPalette) {
    let tier = value_tier(ctx.value);
    let bg = tier.bg(palette);
    let blank = bg_style(bg);
    for dx in 0..ctx.digits {
        write_glyph(buf, ctx.start_x + dx as u16, ctx.cy, ' ', blank);
    }
    if ctx.gap > 0 {
        let gap_x = ctx.start_x + ctx.digits as u16;
        if ctx.sx % 8 == 0 && ctx.x > 0 {
            let style = Style::default()
                .fg(palette.grid)
                .bg(bg)
                .add_modifier(Modifier::DIM);
            write_glyph(buf, gap_x, ctx.cy, '┆', style);
        } else {
            write_glyph(buf, gap_x, ctx.cy, ' ', blank);
        }
    }
    let printable = (0x20u16..=0x7eu16).contains(&ctx.value);
    let fg = if printable {
        ctx.printable_fg
    } else {
        tier.fg(palette)
    };
    let glyphs = to_three_digits(ctx.value);
    let skip = 3 - ctx.digits.min(3);
    let text = Style::default().fg(fg).bg(bg);
    for (dx, &ch) in glyphs.iter().skip(skip).take(ctx.digits).enumerate() {
        write_glyph(buf, ctx.start_x + dx as u16, ctx.cy, ch, text);
    }
}

fn render_bitgrid_bits(area: Rect, buf: &mut Buffer, bits: &SeedBits, on: Color, off: Color) {
    let src_w = bits.width().max(1);
    let src_h = bits.height().max(1);
    let out_w = area.width as usize;
    let out_h = area.height as usize;
    if out_w == 0 || out_h == 0 {
        return;
    }
    for y in 0..out_h {
        let sy = y.saturating_mul(src_h) / out_h;
        for x in 0..out_w {
            let sx = x.saturating_mul(src_w) / out_w;
            let fill = if bits.get(sx, sy) { on } else { off };
            write_glyph(
                buf,
                area.x + x as u16,
                area.y + y as u16,
                ' ',
                bg_style(fill),
            );
        }
    }
}

fn draw_symmetry(area: Rect, buf: &mut Buffer, symmetry: SeedSymmetry, palette: &SeedPalette) {
    let style = Style::default()
        .fg(palette.accent_2)
        .add_modifier(Modifier::DIM);
    let draw_vertical = |buf: &mut Buffer| {
        let mid = area.x + area.width / 2;
        for y in area.y..area.y.saturating_add(area.height) {
            mark_blank_glyph(buf, mid, y, '·', style);
        }
    };
    let draw_horizontal = |buf: &mut Buffer| {
        let mid = area.y + area.height / 2;
        for x in area.x..area.x.saturating_add(area.width) {
            mark_blank_glyph(buf, x, mid, '·', style);
        }
    };
    match symmetry {
        SeedSymmetry::MirrorX => draw_vertical(buf),
        SeedSymmetry::MirrorY => draw_horizontal(buf),
        SeedSymmetry::Rotate180 => {
            draw_vertical(buf);
            draw_horizontal(buf);
        }
        SeedSymmetry::None => {}
    }
}

fn draw_lifehash_inset(area: Rect, buf: &mut Buffer, seed: &EncodedSeed, palette: &SeedPalette) {
    if area.width < INSET_SIZE + 2 || area.height < INSET_SIZE + 2 {
        return;
    }
    let origin_x = area.x + 1;
    let origin_y = area.y + 1;
    for gy in 0..INSET_SIZE as usize {
        for gx in 0..INSET_SIZE as usize {
            let fill = if seed.base_bits_raw.get(gx, gy) {
                palette.accent_2
            } else {
                palette.bg
            };
            write_glyph(
                buf,
                origin_x + gx as u16,
                origin_y + gy as u16,
                ' ',
                bg_style(fill),
            );
        }
    }
    draw_inset_grid(buf, origin_x, origin_y, palette);
    draw_inset_label(buf, origin_x, origin_y, palette);
    let inset_area = Rect {
        x: origin_x,
        y: origin_y,
        width: INSET_SIZE,
        height: INSET_SIZE,
    };
    draw_symmetry(inset_area, buf, seed.params.symmetry, palette);
}

fn draw_inset_grid(buf: &mut Buffer, origin_x: u16, origin_y: u16, palette: &SeedPalette) {
    let style = Style::default()
        .fg(palette.grid)
        .add_modifier(Modifier::DIM);
    for gy in 0..INSET_SIZE {
        for gx in 0..INSET_SIZE {
            if gy % 4 != 0 && gx % 4 != 0 {
                continue;
            }
            mark_blank_glyph(buf, origin_x + gx, origin_y + gy, '·', style);
        }
    }
}

fn draw_hilbert_inset(
    area: Rect,
    buf: &mut Buffer,
    cache: &SeedRenderCache,
    palette: &SeedPalette,
) {
    let Some(inset) = cache.hilbert_path_inset.as_ref() else {
        return;
    };
    if area.width < INSET_SIZE || area.height < INSET_SIZE {
        return;
    }
    let origin_x = area.x + area.width - INSET_SIZE;
    let origin_y = area.y;
    let side = INSET_SIZE as usize;
    for gy in 0..side {
        for gx in 0..side {
            let intensity = inset[gy * side + gx];
            let bg = path_bg(intensity, palette);
            write_glyph(
                buf,
                origin_x + gx as u16,
                origin_y + gy as u16,
                ' ',
                bg_style(bg),
            );
        }
    }
}

fn path_bg(intensity: u8, palette: &SeedPalette) -> Color {
    if intensity > 200 {
        palette.halo_2
    } else if intensity > 120 {
        palette.halo_1
    } else if intensity > 60 {
        palette.grid
    } else {
        palette.bg
    }
}

fn to_three_digits(value: u16) -> [char; 3] {
    [
        (b'0' + (value / 100) as u8) as char,
        (b'0' + ((value / 10) % 10) as u8) as char,
        (b'0' + (value % 10) as u8) as char,
    ]
}

#[derive(Copy, Clone)]
enum ValueTier {
    Low,
    Mid,
    High,
}

fn value_tier(value: u16) -> ValueTier {
    if value >= VALUE_TIER_HIGH {
        ValueTier::High
    } else if value >= VALUE_TIER_MID {
        ValueTier::Mid
    } else {
        ValueTier::Low
    }
}

impl ValueTier {
    fn fg(self, palette: &SeedPalette) -> Color {
        let base = palette.hud_text;
        match self {
            ValueTier::High => mix_towards(palette.accent_2, base, 55),
            ValueTier::Mid => mix_towards(palette.live_dim, base, 40),
            ValueTier::Low => base,
        }
    }

    fn bg(self, palette: &SeedPalette) -> Color {
        match self {
            ValueTier::High => palette.halo_2,
            ValueTier::Mid => palette.halo_1,
            ValueTier::Low => palette.bg,
        }
    }
}

fn mix_towards(top: Color, base: Color, base_pct: u8) -> Color {
    let pct = base_pct.min(100) as u16;
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r0, g0, b0)) = (top, base) else {
        return top;
    };
    let inv = 100u16.saturating_sub(pct);
    let blend = |top: u8, base: u8| -> u8 {
        let top = top as u16;
        let base = base as u16;
        ((top.saturating_mul(inv) + base.saturating_mul(pct) + 50) / 100) as u8
    };
    Color::Rgb(blend(r1, r0), blend(g1, g0), blend(b1, b0))
}

fn gray_from(color: Color, strength_pct: u8) -> Color {
    let pct = strength_pct.min(100) as u16;
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    // Rec. 601 luma, then scale to keep it muted but readable on a dark background.
    let lum = (r as u16 * 30 + g as u16 * 59 + b as u16 * 11 + 50) / 100;
    let scaled = ((lum.saturating_mul(pct) + 50) / 100).min(255) as u8;
    Color::Rgb(scaled, scaled, scaled)
}

pub fn ascii_layout(area_cols: usize, grid_w: usize) -> (usize, usize, usize) {
    if area_cols == 0 || grid_w == 0 {
        return (1, 0, 0);
    }
    let allow_spacing = area_cols >= 4 && (area_cols / 4) >= (grid_w.saturating_add(1) / 2);
    let digits = if allow_spacing || area_cols >= 3 {
        3
    } else if area_cols >= 2 {
        2
    } else {
        1
    };
    let gap = if allow_spacing { 1 } else { 0 };
    let stride = digits + gap;
    let cols = if stride > 0 { area_cols / stride } else { 0 };
    (digits, gap, cols.max(1))
}
