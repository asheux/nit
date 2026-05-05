use serde::{Deserialize, Serialize};

use super::engine::EngineConfig;
use crate::events::EventLogConfig;
use crate::game::{Action, PayoffMatrix};
use crate::strategy::{InputMode, StrategyKind, TmTransition};

#[derive(Clone, Debug)]
pub struct ConfigError {
    pub errors: Vec<String>,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.errors.join("; "))
    }
}

impl std::error::Error for ConfigError {}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct HistoryConfig {
    pub enabled: bool,
    #[serde(default)]
    pub include_cycle_metadata: bool,
}

/// Fully validated and defaulted tournament configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormalizedConfig {
    pub schema_version: u32,
    pub game: String,
    pub rounds: u32,
    pub repetitions: u32,
    pub self_play: bool,
    #[serde(default = "default_save_data")]
    pub save_data: bool,
    pub seed: Option<u64>,
    pub noise: f32,
    pub payoff: PayoffMatrix,
    pub strategies: Vec<StrategySpec>,
    pub event_log: EventLogConfig,
    pub history: HistoryConfig,
    pub engine: EngineConfig,
    #[serde(skip)]
    pub max_memory_n: usize,
    #[serde(skip)]
    pub tm_filter_applied: bool,
}

/// Everything a [`NormalizedConfig`] contains except per-strategy definitions.
/// Family runs sweep a strategy space while holding these base params constant.
#[derive(Clone, Debug)]
pub struct FamilyRunBaseConfig {
    pub schema_version: u32,
    pub game: String,
    pub rounds: u32,
    pub repetitions: u32,
    pub self_play: bool,
    pub save_data: bool,
    pub seed: Option<u64>,
    pub noise: f32,
    pub payoff: PayoffMatrix,
    pub event_log: EventLogConfig,
    pub history: HistoryConfig,
    pub engine: EngineConfig,

    /// Blank symbol hint carried from the strategy section so that Turing
    /// machine family expansion can use the correct blank value.
    pub tm_blank_hint: Option<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategySpec {
    pub id: String,
    pub name: Option<String>,
    #[serde(flatten)]
    pub kind: StrategySpecKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StrategySpecKind {
    Fsm {
        #[serde(default)]
        num_states: usize,
        start_state: usize,
        #[serde(alias = "output")]
        outputs: Vec<Action>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_mode: Option<InputMode>,
        transitions: Vec<Vec<usize>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<u64>,
    },
    Ca {
        n: u64,
        k: u8,
        r: f32,
        t: u32,
    },
    #[serde(rename = "tm", alias = "leftside_tm", alias = "one_sided_tm")]
    OneSidedTm {
        states: u16,
        symbols: u8,
        start_state: u16,
        blank: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        fallback_symbol: Option<u8>,
        max_steps_per_round: u32,
        input_mode: InputMode,
        output_map: Vec<Action>,
        transitions: Vec<TmTransition>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rule_code: Option<u64>,
    },
}

impl StrategySpec {
    pub fn kind_label(&self) -> StrategyKind {
        match self.kind {
            StrategySpecKind::Fsm { .. } => StrategyKind::Fsm,
            StrategySpecKind::Ca { .. } => StrategyKind::Ca,
            StrategySpecKind::OneSidedTm { .. } => StrategyKind::OneSidedTm,
        }
    }
}

pub(in crate::config) const fn default_save_data() -> bool {
    true
}
