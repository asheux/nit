//! Configuration types, normalization, strategy parsing, and payoff handling.

mod normalize;
mod payoff;
mod strategy_parse;
mod types;

pub use types::{
    AcceleratorMode, ComplexityCostConfig, ConfigError, EngineConfig, EngineMode,
    FamilyRunBaseConfig, FsmGroupingMode, GamesConfig, HistoryConfig, NormalizedConfig,
    ParallelismConfig, ParallelismMode, PayoffConfig, ScoreAggregation, StrategyConfig,
    StrategySpec, StrategySpecKind,
};

pub(crate) const CONFIG_SCHEMA_VERSION: u32 = 1;

pub(crate) type ConfigResult<T> = Result<T, ConfigError>;

/// Maps user-facing game aliases to the canonical identifier (e.g. `"pd"` -> `"ipd"`).
pub(crate) fn canonical_game_name(name: &str) -> Option<&'static str> {
    match name {
        "ipd"
        | "pd"
        | "prisoners_dilemma"
        | "prisoners-dilemma"
        | "iterated_prisoners_dilemma"
        | "iterated-prisoners-dilemma" => Some("ipd"),
        _ => None,
    }
}
