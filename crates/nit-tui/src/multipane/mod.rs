//! Multipane launch mode — a grid of independent chat panes, one cwd
//! each, all backed by a single user-chosen agent backend.
//!
//! The `state.multipane.is_some()` invariant gates this module: when
//! `Some`, `app::runner::run` branches into `multipane::run_loop` BEFORE
//! constructing `SyntaxRuntime` / spawning the standard runners. The
//! single-app `run_loop` is never entered in multipane mode.

pub mod agent_id;
pub mod dir_search;
pub mod dir_search_runner;
pub mod dispatch;
pub mod focus;
pub mod grid;
pub mod persistence;
pub mod roster_view;
pub mod selection;
pub mod setup;

mod runtime;

pub use runtime::run_loop;

#[cfg(test)]
#[path = "../tests/multipane_integration.rs"]
mod integration_tests;
