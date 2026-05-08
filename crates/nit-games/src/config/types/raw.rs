use serde::Deserialize;

use super::engine::EngineConfig;
use super::normalized::HistoryConfig;
use crate::events::EventLogConfig;

/// Raw deserialized `games.toml` configuration before normalization.
#[derive(Clone, Debug, Deserialize)]
pub struct GamesConfig {
    pub schema_version: Option<u32>,
    pub game: Option<String>,
    pub rounds: Option<u32>,
    pub repetitions: Option<u32>,
    pub self_play: Option<bool>,
    pub save_data: Option<bool>,
    pub seed: Option<u64>,
    /// Action-flip noise probability in `[0.0, 1.0]`.
    pub noise: Option<f32>,
    pub payoff: Option<PayoffConfig>,
    #[serde(default)]
    pub strategy: Vec<StrategyConfig>,
    pub event_log: Option<EventLogConfig>,
    pub history: Option<HistoryConfig>,
    pub engine: Option<EngineConfig>,
}

/// Either four named PD scalars (`R`, `S`, `T`, `P`) or a full 2x2x2 matrix.
/// If both are provided the matrix takes precedence; the scalars are then
/// validated against the matrix and any mismatch is reported as an error.
#[derive(Clone, Debug, Deserialize)]
pub struct PayoffConfig {
    #[serde(rename = "R")]
    pub r: Option<i32>,
    #[serde(rename = "S")]
    pub s: Option<i32>,
    #[serde(rename = "T")]
    pub t: Option<i32>,
    #[serde(rename = "P")]
    pub p: Option<i32>,
    pub matrix: Option<Vec<Vec<Vec<i32>>>>,
}

/// Superset of all strategy-family parameters parsed from TOML.
/// Only the fields relevant to the declared `type` are used during normalization.
#[derive(Clone, Debug, Deserialize)]
pub struct StrategyConfig {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub name: Option<String>,

    #[serde(alias = "path")]
    pub source: Option<String>,
    pub limit: Option<usize>,

    // FSM fields
    pub index: Option<u64>,
    pub num_states: Option<usize>,
    pub start_state: Option<usize>,
    pub input_index_base: Option<u8>,
    #[serde(alias = "output")]
    pub outputs: Option<Vec<String>>,
    pub input_mode: Option<String>,
    pub transitions: Option<toml::Value>,
    pub k: Option<usize>,

    // CA fields
    pub n: Option<usize>,
    pub r: Option<f32>,
    pub t: Option<u32>,
    pub steps: Option<u32>,

    // TM fields
    pub states: Option<usize>,
    pub symbols: Option<usize>,
    pub blank: Option<usize>,
    #[serde(alias = "fallback")]
    pub fallback_symbol: Option<usize>,
    pub max_steps_per_round: Option<u32>,
    pub output_map: Option<Vec<String>>,
    pub rule_code: Option<u64>,
}

/// Like [`GamesConfig`] but only retains the strategy hints needed for family expansion.
#[derive(Clone, Debug, Deserialize)]
pub(in crate::config) struct FamilyRunParseConfig {
    pub schema_version: Option<u32>,
    pub game: Option<String>,
    pub rounds: Option<u32>,
    pub repetitions: Option<u32>,
    pub self_play: Option<bool>,
    pub save_data: Option<bool>,
    pub seed: Option<u64>,
    pub noise: Option<f32>,
    pub payoff: Option<PayoffConfig>,

    #[serde(default)]
    pub strategy: Vec<FamilyRunStrategyHint>,

    pub event_log: Option<EventLogConfig>,
    pub history: Option<HistoryConfig>,
    pub engine: Option<EngineConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::config) struct FamilyRunStrategyHint {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub blank: Option<usize>,
}
