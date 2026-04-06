//! Configuration types, normalization, strategy parsing, and payoff handling.
//!
//! The config module owns the full pipeline from raw TOML input to the
//! validated [`NormalizedConfig`] that the tournament engine consumes.
//!
//! # Module structure
//!
//! | Sub-module        | Responsibility                                      |
//! |-------------------|-----------------------------------------------------|
//! | [`types`]         | All public structs, enums, and serde definitions    |
//! | [`normalize`]     | Top-level config + strategy kind normalization      |
//! | [`strategy_parse`]| FSM/CA/TM transition and rule table parsing         |
//! | [`payoff`]        | Payoff matrix construction and validation           |

mod normalize;
mod payoff;
mod strategy_parse;
mod types;

// ── Core config types ────────────────────────────────────────────────────────
//
// Re-exported so consumers can `use nit_games::config::GamesConfig` without
// reaching into the `types` sub-module.

pub use types::{
    AcceleratorMode, ComplexityCostConfig, ConfigError, EngineConfig, EngineMode,
    FamilyRunBaseConfig, FsmGroupingMode, GamesConfig, HistoryConfig, NormalizedConfig,
    ParallelismConfig, ParallelismMode, PayoffConfig, ScoreAggregation, StrategyConfig,
    StrategySpec, StrategySpecKind,
};

// ── Constants ────────────────────────────────────────────────────────────────

/// Current schema version for serialized config files.
pub(crate) const CONFIG_SCHEMA_VERSION: u32 = 1;

// ── Result alias ─────────────────────────────────────────────────────────────

/// Convenience alias used throughout the config pipeline.
pub(crate) type ConfigResult<T> = Result<T, ConfigError>;

// ── Game name helpers ───────────────────────────────────────────────────────

/// Maps a user-facing game name (which may be any recognised alias) to the
/// canonical identifier used internally, or returns `None` for unknown games.
///
/// The canonical name for the Iterated Prisoner's Dilemma is `"ipd"`.
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
