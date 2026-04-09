//! Tournament execution engine — match scheduling, GPU acceleration, and result accumulation.

#![forbid(unsafe_code)]

mod accumulator;
mod halting;
mod kernel;
mod metal;
mod runner;
mod schedule;
mod session;
mod types;

pub const TOURNAMENT_FORMAT_VERSION: u32 = 1;
pub const DEFAULT_REPETITIONS: u32 = 1;

pub type MatchResultWithPreview = (MatchResult, Option<MatchHistoryPreview>);
pub type StrategyIndex = usize;
pub type MatchId = u64;

/// Unique undirected pairings in a round-robin schedule.
pub fn round_robin_pair_count(strategy_count: usize, self_play: bool) -> usize {
    if self_play {
        strategy_count * (strategy_count + 1) / 2
    } else {
        strategy_count * strategy_count.saturating_sub(1) / 2
    }
}

pub use halting::{
    select_halting_turing_machine_strategies, try_select_halting_turing_machine_strategies,
    try_select_halting_turing_machine_strategies_with_diagnostics,
};

pub use kernel::{KernelRunMode, TournamentKernel};
pub use runner::TournamentRunner;

pub use metal::{accelerator_preflight, accelerator_run_preflight};

pub use types::{
    MatchHistoryPreview, MatchResult, MatchSnapshot, Parallelism, TmHaltingFilterBackend,
    TmHaltingFilterDiagnostics, TournamentProgress,
};

#[cfg(test)]
pub(crate) use metal::{metal_batch_totals_for_test, metal_policy_probe_for_test};
