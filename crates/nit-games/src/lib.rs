#![forbid(unsafe_code)]

pub mod analysis;
pub mod config;
pub mod events;
pub mod fast_eval;
pub mod fsm_enum;
pub mod game;
pub mod history;
pub mod history_log;
pub mod introspection;
pub mod output;
pub mod strategy;
pub mod tournament;

pub use analysis::{
    analyze_history, AnalysisConfig, AnalysisPaths, HistoryAnalysis, HistoryAnalysisPreview,
    HistoryAnalysisSummary, MatchSummary, StrategySummary, TrajectoryPreview, TrajectorySample,
};
pub use config::{
    AcceleratorMode, ConfigError, EngineConfig, EngineMode, FsmGroupingMode, GamesConfig,
    NormalizedConfig, ParallelismConfig, ParallelismMode, ScoreAggregation, StrategySpec,
};
pub use events::{EventLogConfig, EventWriter, GameEvent};
pub use fast_eval::{CycleMetadata, FastEvalResult, FastStrategyModel};
pub use fsm_enum::{
    canonical_fsm_indices, canonicalize_fsm, enumerate_fsms,
    group_canonical_fsm_indices_by_behavior, group_canonical_fsm_indices_by_behavior_with_mode,
    unique_fsm_behavior_representatives, unique_fsm_behavior_representatives_with_mode,
    FsmDefinition,
};
pub use game::{Action, Outcome, PayoffMatrix};
pub use history::{History, RoundRecord};
pub use history_log::{HistoryWriter, MatchHistory};
pub use introspection::{
    format_strategy_introspection, introspect_strategy, StrategyIntrospection,
    StrategyIntrospectionKind, StrategyIntrospectionParameters, TmTransitionRecord,
};
pub use nit_metal::BatchPolicySource;
pub use output::{
    run_id_from_seed_config, RunLayout, RunPaths, RunSummary, RuntimeAcceleratorBackend,
    RuntimeAcceleratorStats, TournamentResults, RUN_SUMMARY_SCHEMA_VERSION,
};
pub use strategy::{
    decode_fsm_notebook_index, decode_tm_rule_code_wolfram, fsm_count, history_to_input_u64,
    run_one_sided_tm, run_one_sided_tm_from_integer, tm_max_index, CaStrategy, FsmStrategy,
    InputMode, OneSidedTmStrategy, Strategy, StrategyKind, TmMove, TmRunResult, TmRunStats,
    TmStopReason, TmTrace, TmTraceStep, TmTransition,
};
pub use tournament::{
    accelerator_preflight, accelerator_run_preflight, KernelRunMode, MatchHistoryPreview,
    MatchResult, MatchSnapshot, Parallelism, TournamentKernel, TournamentProgress,
    TournamentRunner,
};

#[cfg(test)]
mod tests;
