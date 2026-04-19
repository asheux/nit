use ratatui::style::Color;

use super::palette::GolPalette;
use super::state::{GolRenderConfig, MAX_DECAY};

pub(crate) fn live_color(
    age: u8,
    neighbors: u8,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) -> Color {
    let base = if cfg.age_shading {
        match age {
            0 | 1 => palette.live_new,
            2..=4 => palette.live,
            _ => palette.live_old,
        }
    } else {
        palette.live
    };

    if !cfg.overlay_heat {
        return base;
    }

    match neighbors {
        0..=1 => palette.live_old,
        2 | 3 => base,
        4 | 5 => palette.live,
        _ => palette.live_new,
    }
}

pub(crate) fn trail_color(decay: u8, palette: &GolPalette) -> Color {
    if decay == 0 {
        return palette.bg;
    }
    let steps = palette.trail.len().max(1) as u8;
    let idx = ((decay.saturating_sub(1)) * steps) / MAX_DECAY.max(1);
    let clamped = idx.min((palette.trail.len() - 1) as u8) as usize;
    palette.trail[clamped]
}

pub(crate) fn row_bg(row: usize, cfg: &GolRenderConfig, palette: &GolPalette) -> Color {
    if cfg.scanlines && row % 2 == 1 {
        palette.scanline
    } else {
        palette.bg
    }
}
