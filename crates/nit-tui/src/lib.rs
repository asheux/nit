#![forbid(unsafe_code)]

pub mod app;
pub mod layout;
pub mod system_stats;
pub mod syntax;
pub mod theme;
pub mod widgets;

pub use app::run;
pub use theme::Theme;
