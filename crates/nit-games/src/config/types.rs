use crate::events::EventLogConfig;
use crate::game::{Action, PayoffMatrix};
use crate::strategy::{InputMode, StrategyKind, TmTransition};
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, Deserialize)]
pub struct GamesConfig {
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
    pub strategy: Vec<StrategyConfig>,
    pub event_log: Option<EventLogConfig>,
    pub history: Option<HistoryConfig>,
    pub engine: Option<EngineConfig>,
}

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
    pub tm_blank_hint: Option<u8>,
}

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

#[derive(Clone, Debug, Deserialize)]
pub struct StrategyConfig {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub name: Option<String>,

    #[serde(alias = "path")]
    pub source: Option<String>,
    pub limit: Option<usize>,

    pub index: Option<u64>,
    pub num_states: Option<usize>,
    pub start_state: Option<usize>,
    pub input_index_base: Option<u8>,
    #[serde(alias = "output")]
    pub outputs: Option<Vec<String>>,
    pub input_mode: Option<String>,
    pub transitions: Option<toml::Value>,
    pub k: Option<usize>,

    pub n: Option<usize>,
    pub r: Option<f32>,
    pub t: Option<u32>,
    pub steps: Option<u32>,

    pub states: Option<usize>,
    pub symbols: Option<usize>,
    pub blank: Option<usize>,
    #[serde(alias = "fallback")]
    pub fallback_symbol: Option<usize>,
    pub max_steps_per_round: Option<u32>,
    pub output_map: Option<Vec<String>>,
    pub rule_code: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct FamilyRunParseConfig {
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
pub(super) struct FamilyRunStrategyHint {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub blank: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct HistoryConfig {
    pub enabled: bool,
    #[serde(default)]
    pub include_cycle_metadata: bool,
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineConfig {
    #[serde(default)]
    pub mode: EngineMode,
    #[serde(default)]
    pub parallelism: ParallelismConfig,
    #[serde(default = "default_progress_interval_ms")]
    pub progress_interval_ms: u64,
    #[serde(default = "default_fast_eval")]
    pub fast_eval: bool,
    #[serde(default)]
    pub accelerator: AcceleratorMode,
    #[serde(default)]
    pub score_aggregation: ScoreAggregation,
    #[serde(default)]
    pub fsm_grouping: FsmGroupingMode,
    #[serde(default)]
    pub complexity_cost: ComplexityCostConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            mode: EngineMode::Interactive,
            parallelism: ParallelismConfig::default(),
            progress_interval_ms: default_progress_interval_ms(),
            fast_eval: default_fast_eval(),
            accelerator: AcceleratorMode::default(),
            score_aggregation: ScoreAggregation::default(),
            fsm_grouping: FsmGroupingMode::default(),
            complexity_cost: ComplexityCostConfig::default(),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AcceleratorMode {
    #[default]
    Auto,
    Cpu,
    Metal,
}

impl AcceleratorMode {
    pub fn allows_metal(self) -> bool {
        !matches!(self, Self::Cpu)
    }

    pub fn requires_metal(self) -> bool {
        matches!(self, Self::Metal)
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScoreAggregation {
    #[default]
    #[serde(alias = "average", alias = "avg")]
    Mean,
    #[serde(alias = "sum")]
    Total,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum FsmGroupingMode {
    #[default]
    #[serde(alias = "notebook")]
    Wnbm,
    #[serde(alias = "exact", alias = "moore")]
    Moorem,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComplexityCostConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub tm_step_cost: f64,
    #[serde(default)]
    pub fsm_state_cost: f64,
}

impl Default for ComplexityCostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tm_step_cost: 0.0,
            fsm_state_cost: 0.0,
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EngineMode {
    #[default]
    Interactive,
    Batch,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParallelismConfig {
    Mode(ParallelismMode),
    Threads { threads: usize },
}

impl Default for ParallelismConfig {
    fn default() -> Self {
        ParallelismConfig::Mode(ParallelismMode::Auto)
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParallelismMode {
    Auto,
    Off,
}

pub(super) fn default_progress_interval_ms() -> u64 {
    80
}

pub(super) fn default_fast_eval() -> bool {
    true
}

pub(super) fn default_save_data() -> bool {
    true
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

    pub fn is_deterministic(&self) -> bool {
        true
    }
}

impl StrategySpecKind {
    pub fn is_deterministic(&self) -> bool {
        true
    }
}
