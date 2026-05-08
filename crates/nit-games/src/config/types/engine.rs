use serde::{Deserialize, Serialize};

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
    /// Wrapper-Normal-By-Moore (notebook) canonical form.
    #[default]
    #[serde(alias = "notebook")]
    Wnbm,
    /// Exact Moore-machine canonical form.
    #[serde(alias = "exact", alias = "moore")]
    Moorem,
}

/// Per-step complexity costs subtracted from strategy scores. When `enabled`,
/// each TM step or FSM state incurs the configured penalty.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ComplexityCostConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub tm_step_cost: f64,
    #[serde(default)]
    pub fsm_state_cost: f64,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EngineMode {
    #[default]
    Interactive,
    Batch,
}

/// Either a named preset (`Auto`, `Off`) or an explicit thread count.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParallelismConfig {
    Mode(ParallelismMode),
    Threads { threads: usize },
}

impl Default for ParallelismConfig {
    fn default() -> Self {
        Self::Mode(ParallelismMode::Auto)
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParallelismMode {
    Auto,
    Off,
}

const fn default_progress_interval_ms() -> u64 {
    80
}

const fn default_fast_eval() -> bool {
    true
}
