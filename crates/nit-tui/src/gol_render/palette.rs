use std::sync::OnceLock;

use ratatui::style::Color;

use crate::theme::Theme;

#[derive(Clone, Copy, Debug)]
pub struct GolPalette {
    pub bg: Color,
    pub live_new: Color,
    pub live: Color,
    pub live_old: Color,
    pub trail: [Color; 3],
    pub bbox: Color,
    pub hud_dim: Color,
    pub hud_text: Color,
    pub scanline: Color,
}

impl GolPalette {
    pub fn from_theme(theme: &Theme) -> Self {
        if supports_truecolor() {
            let bg = theme.gol.bg;
            let scanline = darken(bg, 0.88);
            Self {
                bg,
                live_new: theme.gol.live_new,
                live: theme.gol.live,
                live_old: theme.gol.live_old,
                trail: [theme.gol.trail_1, theme.gol.trail_2, theme.gol.trail_3],
                bbox: theme.gol.bbox,
                hud_dim: theme.gol.hud_dim,
                hud_text: theme.gol.hud_text,
                scanline,
            }
        } else {
            Self {
                bg: Color::Black,
                live_new: Color::Cyan,
                live: Color::Cyan,
                live_old: Color::Blue,
                trail: [Color::DarkGray, Color::DarkGray, Color::Black],
                bbox: Color::Cyan,
                hud_dim: Color::DarkGray,
                hud_text: Color::Gray,
                scanline: Color::Black,
            }
        }
    }
}

fn supports_truecolor() -> bool {
    static SUPPORTS: OnceLock<bool> = OnceLock::new();
    *SUPPORTS.get_or_init(|| {
        let colorterm = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
        let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
        colorterm.contains("truecolor")
            || colorterm.contains("24bit")
            || term.contains("truecolor")
            || term.contains("24bit")
    })
}

fn darken(color: Color, factor: f32) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            ((r as f32) * factor) as u8,
            ((g as f32) * factor) as u8,
            ((b as f32) * factor) as u8,
        ),
        other => other,
    }
}
