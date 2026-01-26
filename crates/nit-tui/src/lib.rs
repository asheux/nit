#![forbid(unsafe_code)]

pub mod app;
pub mod gol_render;
pub mod layout;
pub mod petri_dish;
pub mod seed_runtime;
pub mod seed_snapshot;
pub mod system_stats;
pub mod syntax;
pub mod theme;
pub mod widgets;

pub use app::run;
pub use theme::Theme;
