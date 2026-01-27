pub mod braille;
pub mod geometry;
pub mod halfblock;
pub mod palette;
pub mod renderer;
pub mod solid;

pub use ascii_seed::AsciiSeedWidget;
pub use geometry::{RenderGeometry, RenderMode};
pub use palette::GolPalette;
pub use renderer::{
    grid_size_for_mode, AliveHistory, GolHudMetrics, GolHudState, GolRenderConfig,
    GolRenderPipeline, GolRenderState, HUD_HISTORY_LEN, MAX_AGE, MAX_DECAY,
};

use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

use nit_gol::Grid;

pub struct GolWidget<'a> {
    pub grid: &'a Grid,
    pub state: &'a GolRenderState,
    pub cfg: GolRenderConfig,
    pub palette: GolPalette,
    pub hud: GolHudState<'a>,
}

impl<'a> Widget for GolWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut pipeline = GolRenderPipeline::default();
        pipeline.render(
            area,
            buf,
            self.grid,
            self.state,
            &self.cfg,
            &self.palette,
            &self.hud,
        );
    }
}
pub mod ascii_seed;
