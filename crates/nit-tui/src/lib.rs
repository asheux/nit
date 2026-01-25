#![forbid(unsafe_code)]

pub mod app;
pub mod gol_render;
pub mod layout;
pub mod system_stats;
pub mod syntax;
pub mod theme;
pub mod visualizer;
pub mod widgets;

pub use app::run;
pub use theme::Theme;
