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
    pub tissue: Vec<Color>,
}

impl SeedPalette {
    pub fn from_theme(theme: &Theme) -> Self {
        let seed = &theme.seed;
        Self {
            bg: seed.bg,
            live: seed.live,
            live_dim: seed.live_dim,
            halo_1: seed.halo_1,
            halo_2: seed.halo_2,
            grid: seed.grid,
            bbox: seed.bbox,
            hud_text: seed.hud_text,
            hud_dim: seed.hud_dim,
            tissue: seed.tissue_palette.clone(),
        }
    }
}
