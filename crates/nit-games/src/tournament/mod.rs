//! Tournament execution engine — match scheduling, GPU acceleration, and result accumulation.
//!
//! # Architecture
//!
//! The tournament module is split into submodules by responsibility:
//!
//! - **`types`** — shared data types: progress snapshots, match results, seed derivation,
//!   parallelism helpers, and the per-strategy / pairwise statistics structs.
//! - **`schedule`** — deterministic matchup scheduling: converts `(strategy_count,
//!   repetitions, self_play)` into a flat sequence of [`Matchup`](types::Matchup) values.
//! - **`session`** — match lifecycle: strategy construction, round-by-round play
//!   (`play_round_core`), and the batch-mode `run_match_core` entry point.
//! - **`accumulator`** — result aggregation: folds individual match outcomes into
//!   per-strategy rankings and pairwise head-to-head tables.
//! - **`kernel`** — batch executor: runs the entire schedule to completion in a single
//!   call, suitable for CLI and petri-dish evaluations.
//! - **`runner`** — incremental executor: drives the tournament one step at a time for
//!   interactive TUI playback with progress updates between ticks.
//! - **`halting`** — Turing-machine halting filter: pre-screens TM strategies that never
//!   halt within the step budget and removes them before the tournament runs.
//! - **`metal`** — Metal GPU batch evaluation: translates strategy rosters into packed
//!   GPU representations, dispatches evaluation, and converts scores back to outcomes.
//!
//! # Versioning
//!
//! The [`TOURNAMENT_FORMAT_VERSION`] constant is bumped whenever the serialised
//! match-result or progress-snapshot format changes in a backwards-incompatible way.

// ── Submodules ────────────────────────────────────────────────────────────

mod accumulator;
mod halting;
mod kernel;
mod metal;
mod runner;
mod schedule;
mod session;
mod types;

// ── Constants ─────────────────────────────────────────────────────────────

/// Format version for serialised tournament artefacts (match results, progress
/// snapshots).  Bump this when the on-disk layout changes in a way that older
/// readers cannot handle.
pub const TOURNAMENT_FORMAT_VERSION: u32 = 1;

/// Default number of repetitions when the config omits the field.
pub const DEFAULT_REPETITIONS: u32 = 1;

// ── Type aliases ──────────────────────────────────────────────────────────

/// Shorthand for a match result paired with an optional history preview, which
/// is a common return shape when presenting results to the UI layer.
pub type MatchResultWithPreview = (MatchResult, Option<MatchHistoryPreview>);

/// Index of a strategy within the tournament roster (0-based).
pub type StrategyIndex = usize;

/// A monotonically increasing identifier assigned to each match in a schedule.
pub type MatchId = u64;

// ── Utility functions ────────────────────────────────────────────────────

/// Compute the number of unique pairings in a round-robin schedule.
///
/// With `self_play` enabled the formula is `n * (n + 1) / 2`; otherwise
/// it is `n * (n - 1) / 2`.
pub fn round_robin_pair_count(strategy_count: usize, self_play: bool) -> usize {
    if self_play {
        strategy_count * (strategy_count + 1) / 2
    } else {
        strategy_count * strategy_count.saturating_sub(1) / 2
    }
}

// ── Public API: halting filter ────────────────────────────────────────────

pub use halting::{
    select_halting_turing_machine_strategies, try_select_halting_turing_machine_strategies,
    try_select_halting_turing_machine_strategies_with_diagnostics,
};

// ── Public API: execution engines ─────────────────────────────────────────

pub use kernel::{KernelRunMode, TournamentKernel};
pub use runner::TournamentRunner;

// ── Public API: GPU acceleration ──────────────────────────────────────────

pub use metal::{accelerator_preflight, accelerator_run_preflight};

// ── Public API: shared types ──────────────────────────────────────────────

pub use types::{
    MatchHistoryPreview, MatchResult, MatchSnapshot, Parallelism, TmHaltingFilterBackend,
    TmHaltingFilterDiagnostics, TournamentProgress,
};

// ── Test-only re-exports ──────────────────────────────────────────────────

#[cfg(test)]
pub(crate) use metal::{metal_batch_totals_for_test, metal_policy_probe_for_test};
