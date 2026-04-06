//! Tournament execution engine — match scheduling, GPU acceleration, and result accumulation.

mod accumulator;
mod halting;
mod kernel;
mod metal;
mod runner;
mod schedule;
mod session;
mod types;

// ── Public API (preserves the pre-split surface) ───────────────────────────
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

// ── Test-only re-exports ───────────────────────────────────────────────────
#[cfg(test)]
pub(crate) use metal::{metal_batch_totals_for_test, metal_policy_probe_for_test};
