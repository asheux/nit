//! TUI crate: event loop, widgets, agent runners (Claude/Codex), swarm
//! orchestration, and game/seed simulation UIs.
//!
//! No `unsafe` is permitted anywhere in this crate — shell-outs to `codex`,
//! `claude`, and `git` are spawned directly as subprocesses, never via a shell.

#![forbid(unsafe_code)]

pub mod app;
pub mod layout;
pub mod theme;
pub mod widgets;

pub mod claude_runner;
pub mod codex_runner;
pub mod intake;
pub mod multipane;
pub mod shadow;
pub mod swarm;

pub mod file_tree;
pub mod file_tree_runner;
pub mod file_watcher;
pub mod fuzzy_preview_runner;
pub mod fuzzy_search_runner;
pub mod syntax;

pub mod games_analysis;
pub mod games_petri_dish;
pub mod games_runner;
pub mod games_runs;
pub mod gol_render;
pub mod petri_dish;
pub mod seed_render;
pub mod seed_runtime;
pub mod seed_snapshot;

pub mod genome_worker;
pub mod system_stats;
pub mod vitals;
pub mod workspace_scan;

#[cfg(unix)]
pub mod mcp_backchannel;

pub use app::run;
pub use theme::Theme;
