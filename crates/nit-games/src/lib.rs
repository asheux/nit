#![forbid(unsafe_code)]

pub mod config;
pub mod events;
pub mod fast_eval;
pub mod game;
pub mod analysis;
pub mod history;
pub mod history_log;
pub mod output;
pub mod strategy;
pub mod tournament;

pub use config::{
    ConfigError, EngineConfig, EngineMode, GamesConfig, NormalizedConfig, ParallelismConfig,
    ParallelismMode, StrategySpec,
};
pub use analysis::{
    analyze_history, AnalysisConfig, AnalysisPaths, HistoryAnalysis, HistoryAnalysisPreview,
    HistoryAnalysisSummary, MatchSummary, StrategySummary, TrajectoryPreview, TrajectorySample,
};
pub use events::{EventLogConfig, EventWriter, GameEvent};
pub use fast_eval::{CycleMetadata, FastEvalResult, FastStrategyModel};
pub use game::{Action, Outcome, PayoffMatrix};
pub use history::{History, RoundRecord};
pub use history_log::{HistoryWriter, MatchHistory};
pub use output::{
    run_id_from_seed_config, RunLayout, RunPaths, RunSummary, TournamentResults,
    RUN_SUMMARY_SCHEMA_VERSION,
};
pub use strategy::{
    AlwaysCooperate, AlwaysDefect, FsmStrategy, GrimTrigger, MemoryStrategy, RandomStrategy,
    Strategy, StrategyKind, TitForTat, WinStayLoseShift,
};
pub use tournament::{
    KernelRunMode, MatchResult, MatchSnapshot, Parallelism, TournamentKernel, TournamentProgress,
    TournamentRunner,
};

#[cfg(test)]
mod tests;
