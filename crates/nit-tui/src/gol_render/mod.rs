pub mod ascii_seed;
pub mod braille;
pub mod color;
pub mod geometry;
pub mod halfblock;
pub mod hud;
pub mod overlay;
pub mod palette;
pub mod renderer;
pub mod solid;
pub mod state;

pub use ascii_seed::AsciiSeedWidget;
pub use geometry::{RenderGeometry, RenderMode};
pub use palette::GolPalette;
pub use renderer::{grid_size_for_mode, GolRenderPipeline, GolWidget};
pub use state::{
    AliveHistory, GolHudMetrics, GolHudState, GolRenderConfig, GolRenderState, HUD_HISTORY_LEN,
    MAX_AGE, MAX_DECAY,
};
