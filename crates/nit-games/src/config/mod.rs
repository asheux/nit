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

pub(crate) fn is_tm_kind(kind: &str) -> bool {
    matches!(
        kind,
        "leftside_tm"
            | "left-side-tm"
            | "one_sided_tm"
            | "one-sided-tm"
            | "one_sided_tm_strategy"
            | "tm"
            | "onesidedtm"
    )
}

pub(crate) fn normalize_kind_str(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_ascii_lowercase())
}
