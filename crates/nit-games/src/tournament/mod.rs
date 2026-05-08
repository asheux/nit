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

pub type MatchResultWithPreview = (MatchResult, Option<MatchHistoryPreview>);
pub type StrategyIndex = usize;
pub type MatchId = u64;

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
pub use metal::{accelerator_preflight, accelerator_run_preflight};
pub use runner::TournamentRunner;
pub use types::{
    MatchHistoryPreview, MatchResult, MatchSnapshot, Parallelism, TmHaltingFilterBackend,
    TmHaltingFilterDiagnostics, TournamentProgress,
};

#[cfg(test)]
pub(crate) use session::build_strategy;

// Test-only re-exports so `test_modules/` files (which live OUTSIDE the
// `tournament` module privacy boundary) can poke at internal types.
// Removing these breaks the test build on macOS.
#[cfg(test)]
pub(crate) mod test_internals {
    pub(crate) use super::metal::{
        encode_matchup_pairs, match_outcomes_from_scores,
        try_metal_batch_outcomes_chunked_prepared, try_prepare_metal_batch,
    };
    pub(crate) use super::types::{MatchOutcome, Matchup, PreparedMetalBatch};
}
