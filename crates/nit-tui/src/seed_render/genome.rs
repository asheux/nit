use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use nit_core::seed::SeedBits;
use nit_core::{EncodedSeed, SeedEncoderId};

use super::palette::SeedPalette;
use super::renderer::SeedRenderCache;

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
        SeedEncoderId::HilbertBits => render_hilbert_bits(area, buf, seed, cache, palette),
        SeedEncoderId::AsciiBytes => render_ascii_bytes(area, buf, seed, palette),
    }
}

fn render_lifehash16(area: Rect, buf: &mut Buffer, seed: &EncodedSeed, palette: &SeedPalette) {
    render_bitgrid_bits(area, buf, &seed.base_bits_raw, palette.accent_2, palette.bg);
    draw_symmetry(area, buf, seed, palette);
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
        render_bitgrid(area, buf, seed, palette.accent_2, palette.bg);
        return;
    };
    let cols = area.width as usize;
    let rows = area.height as usize;
    if cols == 0 || rows == 0 {
        return;
    }
    let total = stream.len().max(1);
    let mut stride = total.div_ceil(cols);
    if stride == 0 {
        stride = 1;
    }
    for x in 0..cols {
        let idx = x.saturating_mul(stride);
        let bit = idx < total && stream[idx] != 0;
        let bg = if bit { palette.accent_2 } else { palette.bg };
        let sep = idx % 64 == 0 || idx % 16 == 0 || idx % 8 == 0;
        let sep_char = if idx % 64 == 0 {
            '┆'
        } else if idx % 16 == 0 {
            '┊'
        } else if idx % 8 == 0 {
            '│'
        } else {
            ' '
        };
        let sep_style = Style::default()
            .fg(palette.grid)
            .bg(bg)
            .add_modifier(Modifier::DIM);
        let fill_style = Style::default().bg(bg);
        for y in 0..rows {
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            if sep {
                cell.set_char(sep_char);
                cell.set_style(sep_style);
            } else {
                cell.set_char(' ');
                cell.set_style(fill_style);
            }
        }
    }
    draw_hilbert_inset(area, buf, cache, palette);
}

fn render_ascii_bytes(area: Rect, buf: &mut Buffer, seed: &EncodedSeed, palette: &SeedPalette) {
    let w = seed.base_values.width().max(1);
    let h = seed.base_values.height().max(1);
    let max_cols = area.width as usize;
    let max_rows = area.height as usize;
    if max_cols == 0 || max_rows == 0 {
        return;
    }
    let (digits, gap, cols) = ascii_layout(max_cols, w);
    if cols == 0 {
        return;
    }
    // Keep genome byte colors in the cyan family so the view doesn't become a rainbow of accents.
    // Reserve the full accent yellow for selection/crosshair.
    let printable_fg = mix_towards(palette.accent, palette.hud_text, 65);
    let rows = max_rows.max(1);
    let stride = digits + gap;
    for y in 0..rows {
        let sy = y.saturating_mul(h) / rows;
        for x in 0..cols {
            let sx = x.saturating_mul(w) / cols;
            let value = seed.base_values.get(sx, sy) as u16;
            let printable = (0x20u16..=0x7eu16).contains(&value);
            let start_x = area.x + (x * stride) as u16;
            let bg = value_bg(value, palette);
            for dx in 0..digits {
                let cell = buf.get_mut(start_x + dx as u16, area.y + y as u16);
                cell.set_char(' ');
                cell.set_style(Style::default().bg(bg));
            }
            if gap > 0 {
                let gap_x = start_x + digits as u16;
                let cell = buf.get_mut(gap_x, area.y + y as u16);
                if sx % 8 == 0 && x > 0 {
                    cell.set_char('┆');
                    cell.set_style(
                        Style::default()
                            .fg(palette.grid)
                            .bg(bg)
                            .add_modifier(Modifier::DIM),
                    );
                } else {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }
            let digits_arr = to_three_digits(value);
            let fg = if printable {
                printable_fg
            } else {
                value_fg(value, palette)
            };
            let start_idx = match digits {
                1 => 2,
                2 => 1,
                _ => 0,
            };
            for (dx, &ch) in digits_arr.iter().skip(start_idx).take(digits).enumerate() {
                let cell = buf.get_mut(start_x + dx as u16, area.y + y as u16);
                cell.set_char(ch);
                cell.set_style(Style::default().fg(fg).bg(bg));
            }
        }
    }
}

fn render_bitgrid(
    area: Rect,
    buf: &mut Buffer,
    seed: &EncodedSeed,
    on: ratatui::style::Color,
    off: ratatui::style::Color,
) {
    render_bitgrid_bits(area, buf, &seed.base_bits, on, off);
}

fn render_bitgrid_bits(
    area: Rect,
    buf: &mut Buffer,
    bits: &SeedBits,
    on: ratatui::style::Color,
    off: ratatui::style::Color,
) {
    let w = bits.width().max(1);
    let h = bits.height().max(1);
    let out_w = area.width as usize;
    let out_h = area.height as usize;
    if out_w == 0 || out_h == 0 {
        return;
    }
    for y in 0..out_h {
        let sy = y.saturating_mul(h) / out_h;
        for x in 0..out_w {
            let sx = x.saturating_mul(w) / out_w;
            let alive = bits.get(sx, sy);
            let cell = buf.get_mut(area.x + x as u16, area.y + y as u16);
            cell.set_char(' ');
            cell.set_style(Style::default().bg(if alive { on } else { off }));
        }
    }
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

fn draw_lifehash_inset(area: Rect, buf: &mut Buffer, seed: &EncodedSeed, palette: &SeedPalette) {
    let inset_w = 16u16;
    let inset_h = 16u16;
    if area.width < inset_w + 2 || area.height < inset_h + 2 {
        return;
    }
    let inset_x = area.x + 1;
    let inset_y = area.y + 1;
    for y in 0..16usize {
        for x in 0..16usize {
            let alive = seed.base_bits_raw.get(x, y);
            let cell = buf.get_mut(inset_x + x as u16, inset_y + y as u16);
            cell.set_char(' ');
            cell.set_style(Style::default().bg(if alive { palette.accent_2 } else { palette.bg }));
        }
    }
    draw_inset_grid(inset_x, inset_y, palette, buf);
    draw_inset_label(inset_x, inset_y, palette, buf);
    draw_symmetry(
        Rect {
            x: inset_x,
            y: inset_y,
            width: inset_w,
            height: inset_h,
        },
        buf,
        seed,
        palette,
    );
}

fn draw_inset_grid(inset_x: u16, inset_y: u16, palette: &SeedPalette, buf: &mut Buffer) {
    let style = Style::default()
        .fg(palette.grid)
        .add_modifier(Modifier::DIM);
    for y in 0..16u16 {
        if y % 4 == 0 {
            for x in 0..16u16 {
                let cell = buf.get_mut(inset_x + x, inset_y + y);
                if cell.symbol() == " " {
                    cell.set_char('·');
                    cell.set_style(style);
                }
            }
        }
    }
    for x in 0..16u16 {
        if x % 4 == 0 {
            for y in 0..16u16 {
                let cell = buf.get_mut(inset_x + x, inset_y + y);
                if cell.symbol() == " " {
                    cell.set_char('·');
                    cell.set_style(style);
                }
            }
        }
    }
}

fn draw_inset_label(inset_x: u16, inset_y: u16, palette: &SeedPalette, buf: &mut Buffer) {
    let label = "INSET";
    let style = Style::default()
        .fg(palette.hud_dim)
        .add_modifier(Modifier::DIM);
    let y = inset_y.saturating_sub(1);
    let mut x = inset_x;
    for ch in label.chars() {
        let cell = buf.get_mut(x, y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
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
    let inset_w = 16u16;
    let inset_h = 16u16;
    if area.width < inset_w || area.height < inset_h {
        return;
    }
    let inset_x = area.x + area.width - inset_w;
    let inset_y = area.y;
    for y in 0..16usize {
        for x in 0..16usize {
            let v = inset[y * 16 + x];
            let bg = path_bg(v, palette);
            let cell = buf.get_mut(inset_x + x as u16, inset_y + y as u16);
            cell.set_char(' ');
            cell.set_style(Style::default().bg(bg));
        }
    }
}

fn path_bg(value: u8, palette: &SeedPalette) -> ratatui::style::Color {
    if value > 200 {
        palette.halo_2
    } else if value > 120 {
        palette.halo_1
    } else if value > 60 {
        palette.grid
    } else {
        palette.bg
    }
}

fn to_three_digits(value: u16) -> [char; 3] {
    let hundreds = (value / 100) as u8;
    let tens = ((value / 10) % 10) as u8;
    let ones = (value % 10) as u8;
    [
        (b'0' + hundreds) as char,
        (b'0' + tens) as char,
        (b'0' + ones) as char,
    ]
}

fn value_fg(value: u16, palette: &SeedPalette) -> ratatui::style::Color {
    let base = palette.hud_text;
    if value >= 200 {
        mix_towards(palette.accent_2, base, 55)
    } else if value >= 120 {
        mix_towards(palette.live_dim, base, 40)
    } else {
        base
    }
}

fn value_bg(value: u16, palette: &SeedPalette) -> ratatui::style::Color {
    if value >= 200 {
        palette.halo_2
    } else if value >= 120 {
        palette.halo_1
    } else {
        palette.bg
    }
}

fn mix_towards(
    color: ratatui::style::Color,
    target: ratatui::style::Color,
    target_pct: u8,
) -> ratatui::style::Color {
    let pct = (target_pct.min(100)) as u16;
    match (color, target) {
        (ratatui::style::Color::Rgb(r1, g1, b1), ratatui::style::Color::Rgb(r0, g0, b0)) => {
            let inv = 100u16.saturating_sub(pct);
            let mix = |top: u8, base: u8| -> u8 {
                let top = top as u16;
                let base = base as u16;
                ((top.saturating_mul(inv) + base.saturating_mul(pct) + 50) / 100) as u8
            };
            ratatui::style::Color::Rgb(mix(r1, r0), mix(g1, g0), mix(b1, b0))
        }
        _ => color,
    }
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
