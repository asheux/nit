use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use nit_core::VisualizerMode;

use super::palette::GolPalette;
use super::state::{AliveHistory, GolHudState};

pub(crate) fn render_hud_line(
    area: Rect,
    buf: &mut Buffer,
    palette: &GolPalette,
    hud: &GolHudState<'_>,
) {
    if area.height == 0 {
        return;
    }
    let y = area.y;
    let max_x = area.x.saturating_add(area.width);
    let label_style = Style::default()
        .fg(palette.hud_dim)
        .bg(palette.bg)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(palette.hud_text).bg(palette.bg);
    let sep_style = Style::default()
        .fg(palette.hud_dim)
        .bg(palette.bg)
        .add_modifier(Modifier::DIM);

    for x in area.x..max_x {
        let cell = buf.get_mut(x, y);
        cell.set_char(' ');
        cell.set_bg(palette.bg);
        cell.set_fg(palette.hud_dim);
    }

    let mut x = area.x;
    x = write_str(buf, x, y, max_x, label_style, "Rule: ");
    x = write_str(buf, x, y, max_x, value_style, hud.rule);
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Gen: ");
    x = write_u64(buf, x, y, max_x, value_style, hud.generation, 5);
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Alive: ");
    x = write_usize(buf, x, y, max_x, value_style, hud.alive, 4);
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Δ (changes): ");
    x = write_u32(buf, x, y, max_x, value_style, hud.delta, 3);
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Period: ");
    if let Some(period) = hud.period {
        x = write_u32(buf, x, y, max_x, value_style, period, 2);
    } else {
        x = write_str(buf, x, y, max_x, value_style, "--");
    }
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Mode: ");
    let mode_label = match hud.mode {
        VisualizerMode::SimOnly => "SIM",
        VisualizerMode::Search => "SEARCH",
    };
    x = write_str(buf, x, y, max_x, value_style, mode_label);
    if hud.paused {
        x = write_str(buf, x, y, max_x, sep_style, " PAUSED");
    }

    if x < max_x.saturating_sub(2) {
        x = write_str(buf, x, y, max_x, sep_style, " | ");
        let spark_style = Style::default().fg(palette.hud_spark).bg(palette.bg);
        let _ = write_sparkline(buf, x, y, max_x, spark_style, hud.history);
    }
}

pub(crate) fn write_str(
    buf: &mut Buffer,
    mut x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    text: &str,
) -> u16 {
    for ch in text.chars() {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    x
}

pub(crate) fn write_u64(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    value: u64,
    min_width: usize,
) -> u16 {
    write_number(buf, x, y, max_x, style, value, min_width)
}

pub(crate) fn write_u32(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    value: u32,
    min_width: usize,
) -> u16 {
    write_number(buf, x, y, max_x, style, value as u64, min_width)
}

pub(crate) fn write_usize(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    value: usize,
    min_width: usize,
) -> u16 {
    write_number(buf, x, y, max_x, style, value as u64, min_width)
}

fn write_number(
    buf: &mut Buffer,
    mut x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    mut value: u64,
    min_width: usize,
) -> u16 {
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    if value == 0 {
        digits[len] = b'0';
        len += 1;
    } else {
        while value > 0 {
            digits[len] = b'0' + (value % 10) as u8;
            len += 1;
            value /= 10;
        }
    }
    let pad = min_width.saturating_sub(len);
    for _ in 0..pad {
        if x >= max_x {
            return x;
        }
        let cell = buf.get_mut(x, y);
        cell.set_char('0');
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    for idx in (0..len).rev() {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, y);
        cell.set_char(digits[idx] as char);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    x
}

fn write_sparkline(
    buf: &mut Buffer,
    mut x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    history: &AliveHistory,
) -> u16 {
    let len = history.len();
    if len == 0 {
        return x;
    }
    let mut min = u16::MAX;
    let mut max = 0u16;
    for i in 0..len {
        if let Some(val) = history.get(i) {
            min = min.min(val);
            max = max.max(val);
        }
    }
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    for i in 0..len {
        if x >= max_x {
            break;
        }
        let value = history.get(i).unwrap_or(0);
        let level = if max == min {
            4
        } else {
            let span = (max - min).max(1) as u32;
            let scaled = ((value.saturating_sub(min)) as u32 * 7) / span;
            scaled as usize
        };
        let ch = bars[level.min(7)];
        let cell = buf.get_mut(x, y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    x
}
