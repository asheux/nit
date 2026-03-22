#![forbid(unsafe_code)]

pub mod app;
pub mod claude_runner;
pub mod codex_runner;
pub mod file_tree;
pub mod file_tree_runner;
pub mod fuzzy_preview_runner;
pub mod fuzzy_search_runner;
pub mod games_analysis;
pub mod games_petri_dish;
pub mod games_runner;
pub mod games_runs;
pub mod gol_render;
pub mod layout;
pub mod petri_dish;
pub mod seed_render;
pub mod seed_runtime;
pub mod seed_snapshot;
pub mod swarm;
pub mod syntax;
pub mod system_stats;
pub mod theme;
pub mod vitals;
pub mod widgets;

pub use app::run;
pub use theme::Theme;
