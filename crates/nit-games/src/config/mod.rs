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

const IPD_ALIASES: &[&str] = &[
    "ipd",
    "pd",
    "prisoners_dilemma",
    "prisoners-dilemma",
    "iterated_prisoners_dilemma",
    "iterated-prisoners-dilemma",
];

const TM_ALIASES: &[&str] = &[
    "leftside_tm",
    "left-side-tm",
    "one_sided_tm",
    "one-sided-tm",
    "one_sided_tm_strategy",
    "tm",
    "onesidedtm",
];

pub(crate) fn canonical_game_name(name: &str) -> Option<&'static str> {
    IPD_ALIASES.contains(&name).then_some("ipd")
}

pub(crate) fn is_tm_kind(kind: &str) -> bool {
    TM_ALIASES.contains(&kind)
}

pub(crate) fn normalize_kind_str(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_ascii_lowercase)
}
