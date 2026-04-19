use ratatui::style::Color;

use crate::theme::Theme;

#[derive(Clone, Debug)]
pub struct SeedPalette {
    pub bg: Color,
    pub live: Color,
    pub live_dim: Color,
    pub halo_1: Color,
    pub halo_2: Color,
    pub grid: Color,
    pub bbox: Color,
    pub hud_text: Color,
    pub hud_dim: Color,
    pub accent: Color,
    pub accent_2: Color,
    pub tissue: Vec<Color>,
}

impl SeedPalette {
    pub fn from_theme(theme: &Theme) -> Self {
        let s = &theme.seed;
        Self {
            bg: s.bg,
            live: s.live,
            live_dim: s.live_dim,
            halo_1: s.halo_1,
            halo_2: s.halo_2,
            grid: s.grid,
            bbox: s.bbox,
            hud_text: s.hud_text,
            hud_dim: s.hud_dim,
            accent: s.accent,
            accent_2: s.accent_2,
            tissue: s.tissue_palette.clone(),
        }
    }
}
