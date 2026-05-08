//! Configuration type surface: engine flags, raw TOML structs, and the
//! validated/normalized output produced by `config::normalize`.

mod engine;
mod normalized;
mod raw;

pub use engine::{
    AcceleratorMode, ComplexityCostConfig, EngineConfig, EngineMode, FsmGroupingMode,
    ParallelismConfig, ParallelismMode, ScoreAggregation,
};
pub use normalized::{
    ConfigError, FamilyRunBaseConfig, HistoryConfig, NormalizedConfig, StrategySpec,
    StrategySpecKind,
};
pub use raw::{GamesConfig, PayoffConfig, StrategyConfig};

pub(in crate::config) use normalized::default_save_data;
pub(in crate::config) use raw::{FamilyRunParseConfig, FamilyRunStrategyHint};
