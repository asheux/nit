//! Game theory tournament engine with configurable strategies, payoff matrices, and analysis.
//!
//! This crate provides a complete iterated game theory toolkit: configurable
//! strategy types (FSM, cellular automata, one-sided Turing machines), fast
//! cycle-detecting evaluation, round-robin tournament scheduling with optional
//! GPU acceleration, and post-hoc analysis of match histories.
//!
//! # Crate version
//!
//! [`CRATE_VERSION`] mirrors `Cargo.toml` and can be embedded in serialised
//! artefacts for provenance tracking.

#![forbid(unsafe_code)]

/// Semver version string for `nit-games`, sourced from `Cargo.toml` at compile
/// time.  Useful for embedding in serialised run summaries and log headers.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Modules: domain primitives ────────────────────────────────────────────

/// Core game types: actions, outcomes, and payoff matrices.
pub mod game;

/// Round-by-round match history and record types.
pub mod history;

/// Fast cycle-detecting FSM-vs-FSM evaluation engine.
pub mod fast_eval;

// ── Modules: strategy definitions ─────────────────────────────────────────

/// Strategy trait, concrete strategy implementations, and Turing machine helpers.
pub mod strategy;

/// Exhaustive FSM enumeration, canonicalisation, and behavioural grouping.
pub mod fsm_enum;

// ── Modules: configuration ────────────────────────────────────────────────

/// Configuration parsing, normalisation, and validation.
pub mod config;

// ── Modules: execution ────────────────────────────────────────────────────

/// Round-robin tournament scheduling, execution, and GPU acceleration.
pub mod tournament;

// ── Modules: analysis and output ──────────────────────────────────────────

/// Post-hoc analysis of match histories and trajectory sampling.
pub mod analysis;

/// Strategy introspection and human-readable formatting.
pub mod introspection;

/// Structured NDJSON event logging during tournament runs.
pub mod events;

/// Match history serialisation (compact NDJSON log).
pub mod history_log;

/// Run layout, summary, and results serialisation.
pub mod output;

// ── Re-exports: Core types ────────────────────────────────────────────────

pub use game::{Action, Outcome, PayoffMatrix, ACTION_COUNT, OUTCOME_COUNT};
pub use history::{History, RoundRecord, MAX_ROLLING_DEPTH};

// ── Re-exports: Configuration ─────────────────────────────────────────────

pub use config::{
    AcceleratorMode, ConfigError, EngineConfig, EngineMode, FsmGroupingMode, GamesConfig,
    NormalizedConfig, ParallelismConfig, ParallelismMode, ScoreAggregation, StrategySpec,
};

// ── Re-exports: Strategies ────────────────────────────────────────────────

pub use strategy::{
    decode_fsm_notebook_index, decode_tm_rule_code_wolfram, fsm_count, history_to_input_u64,
    run_one_sided_tm, run_one_sided_tm_from_integer, tm_max_index, CaStrategy, FsmStrategy,
    InputMode, OneSidedTmStrategy, Strategy, StrategyKind, TmMove, TmRunResult, TmRunStats,
    TmStopReason, TmTrace, TmTraceStep, TmTransition,
};

// ── Re-exports: FSM enumeration ───────────────────────────────────────────

pub use fsm_enum::{
    canonical_fsm_indices, canonicalize_fsm, enumerate_fsms,
    group_canonical_fsm_indices_by_behavior, group_canonical_fsm_indices_by_behavior_with_mode,
    unique_fsm_behavior_representatives, unique_fsm_behavior_representatives_with_mode,
    FsmDefinition,
};

// ── Re-exports: Fast evaluation ───────────────────────────────────────────

pub use fast_eval::{CycleMetadata, FastEvalResult, FastStrategyModel};

// ── Re-exports: Tournament engine ─────────────────────────────────────────

pub use tournament::{
    accelerator_preflight, accelerator_run_preflight, round_robin_pair_count,
    select_halting_turing_machine_strategies, try_select_halting_turing_machine_strategies,
    try_select_halting_turing_machine_strategies_with_diagnostics, KernelRunMode,
    MatchHistoryPreview, MatchId, MatchResult, MatchSnapshot, Parallelism, StrategyIndex,
    TmHaltingFilterBackend, TmHaltingFilterDiagnostics, TournamentKernel, TournamentProgress,
    TournamentRunner, TOURNAMENT_FORMAT_VERSION,
};

// ── Re-exports: Analysis ──────────────────────────────────────────────────

pub use analysis::{
    analyze_history, AnalysisConfig, AnalysisPaths, HistoryAnalysis, HistoryAnalysisPreview,
    HistoryAnalysisSummary, MatchSummary, StrategySummary, TrajectoryPreview, TrajectorySample,
};

// ── Re-exports: Introspection ─────────────────────────────────────────────

pub use introspection::{
    format_strategy_introspection, introspect_strategy, StrategyIntrospection,
    StrategyIntrospectionKind, StrategyIntrospectionParameters, TmTransitionRecord,
};

// ── Re-exports: Logging and output ────────────────────────────────────────

pub use events::{EventLogConfig, EventWriter, GameEvent};
pub use history_log::{HistoryWriter, MatchHistory};
pub use nit_metal::BatchPolicySource;
pub use output::{
    run_id_from_seed_config, RunLayout, RunPaths, RunSummary, RuntimeAcceleratorBackend,
    RuntimeAcceleratorStats, TournamentResults, RUN_SUMMARY_SCHEMA_VERSION,
};

#[cfg(test)]
mod tests;
