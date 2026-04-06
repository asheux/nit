// ---------------------------------------------------------------------------
// Configuration types for the nit-games tournament engine.
//
// This module defines the raw deserialized config structs (`GamesConfig`,
// `StrategyConfig`, etc.), their fully-validated counterparts
// (`NormalizedConfig`, `StrategySpec`), and all engine/parallelism/accelerator
// enums that parameterize tournament execution.
// ---------------------------------------------------------------------------

use serde::{Deserialize, Serialize};

use crate::events::EventLogConfig;
use crate::game::{Action, PayoffMatrix};
use crate::strategy::{InputMode, StrategyKind, TmTransition};

// ---- Error types ----------------------------------------------------------

/// Accumulates one or more validation errors encountered during config
/// normalization.  Displayed as a semicolon-separated list.
#[derive(Clone, Debug)]
pub struct ConfigError {
    /// Individual error messages collected during validation.
    pub errors: Vec<String>,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.errors.join("; "))
    }
}

impl std::error::Error for ConfigError {}

// ---- Raw (deserialized) config structs ------------------------------------

/// Raw deserialized representation of a games TOML configuration file.
///
/// All fields are optional at this stage; validation and defaulting happen
/// during normalization (see [`Self::normalize`] / [`Self::normalize_with_root`]).
#[derive(Clone, Debug, Deserialize)]
pub struct GamesConfig {
    /// Config schema version for forward-compatibility checks.
    pub schema_version: Option<u32>,

    /// Game type identifier (e.g. `"prisoners_dilemma"`).
    pub game: Option<String>,

    /// Number of rounds per match.
    pub rounds: Option<u32>,

    /// Number of times each match-up is repeated.
    pub repetitions: Option<u32>,

    /// Whether strategies play against themselves.
    pub self_play: Option<bool>,

    /// Whether to persist match data to disk.
    pub save_data: Option<bool>,

    /// Optional deterministic seed for the RNG.
    pub seed: Option<u64>,

    /// Action-flip noise probability in `[0.0, 1.0]`.
    pub noise: Option<f32>,

    /// Custom payoff matrix overrides.
    pub payoff: Option<PayoffConfig>,

    /// Per-strategy definitions (zero or more).
    #[serde(default)]
    pub strategy: Vec<StrategyConfig>,

    /// Event logging settings.
    pub event_log: Option<EventLogConfig>,

    /// History / cycle-detection settings.
    pub history: Option<HistoryConfig>,

    /// Engine-level execution settings.
    pub engine: Option<EngineConfig>,
}

/// Shared tournament parameters for a family run -- everything a
/// [`NormalizedConfig`] contains except the per-strategy definitions.
///
/// Family runs sweep across a strategy space while holding these base
/// parameters constant.
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

/// User-facing payoff matrix configuration.
///
/// Supports either the four named PD parameters (`R`, `S`, `T`, `P`) or a
/// full 2x2x2 matrix.  If both are provided the matrix takes precedence
/// during normalization.
#[derive(Clone, Debug, Deserialize)]
pub struct PayoffConfig {
    /// Reward for mutual cooperation.
    #[serde(rename = "R")]
    pub r: Option<i32>,

    /// Sucker's payoff (cooperator vs. defector).
    #[serde(rename = "S")]
    pub s: Option<i32>,

    /// Temptation to defect (defector vs. cooperator).
    #[serde(rename = "T")]
    pub t: Option<i32>,

    /// Punishment for mutual defection.
    #[serde(rename = "P")]
    pub p: Option<i32>,

    /// Full 2x2x2 payoff matrix override.
    pub matrix: Option<Vec<Vec<Vec<i32>>>>,
}

/// Per-strategy configuration block as parsed from TOML.
///
/// This is a superset of all strategy-family parameters.  Only the fields
/// relevant to the declared `type` are used; extras are silently ignored
/// during normalization.
#[derive(Clone, Debug, Deserialize)]
pub struct StrategyConfig {
    /// Unique identifier for the strategy within the tournament.
    pub id: String,

    /// Strategy family discriminator (`"fsm"`, `"ca"`, `"tm"`, etc.).
    #[serde(rename = "type")]
    pub kind: Option<String>,

    /// Optional human-readable display name.
    pub name: Option<String>,

    // -- Source / enumeration fields ----------------------------------------
    /// Path to an external strategy definition file or directory.
    #[serde(alias = "path")]
    pub source: Option<String>,

    /// Maximum number of strategies to load from a family enumeration.
    pub limit: Option<usize>,

    // -- FSM-specific fields -----------------------------------------------
    /// Numeric index used for FSM enumeration.
    pub index: Option<u64>,

    /// Total number of FSM states.
    pub num_states: Option<usize>,

    /// Initial state for the FSM.
    pub start_state: Option<usize>,

    /// Base offset for input indexing.
    pub input_index_base: Option<u8>,

    /// Output actions per state (e.g. `["C", "D"]`).
    #[serde(alias = "output")]
    pub outputs: Option<Vec<String>>,

    /// How the FSM reads history (`"outcome"` or `"opponent_action"`).
    pub input_mode: Option<String>,

    /// FSM transition table (parsed from nested TOML arrays).
    pub transitions: Option<toml::Value>,

    /// Neighbourhood radius for cellular-automaton strategies.
    pub k: Option<usize>,

    // -- Cellular-automaton fields ------------------------------------------
    /// Rule length or neighbourhood size for CA enumeration.
    pub n: Option<usize>,

    /// CA radius parameter.
    pub r: Option<f32>,

    /// Number of CA time steps.
    pub t: Option<u32>,

    /// Alias for step count used in some CA configs.
    pub steps: Option<u32>,

    // -- Turing-machine fields ---------------------------------------------
    /// Number of TM states.
    pub states: Option<usize>,

    /// Number of tape symbols.
    pub symbols: Option<usize>,

    /// Index of the blank symbol on the tape.
    pub blank: Option<usize>,

    /// Fallback symbol written when input is unmapped.
    #[serde(alias = "fallback")]
    pub fallback_symbol: Option<usize>,

    /// Hard step limit per round for TM execution.
    pub max_steps_per_round: Option<u32>,

    /// Mapping from tape symbols to player actions.
    pub output_map: Option<Vec<String>>,

    /// Compact numeric encoding of the TM rule table.
    pub rule_code: Option<u64>,
}

// ---- Family-run intermediate parse types ----------------------------------

/// Intermediate deserialized form for a family-run config file.
///
/// Similar to [`GamesConfig`] but carries only the minimal strategy hints
/// needed to bootstrap family expansion (e.g. strategy type and blank symbol).
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

/// Minimal strategy metadata extracted during family-run parsing.
///
/// Only the strategy type and blank symbol are needed to resolve the family
/// before full strategy expansion occurs.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct FamilyRunStrategyHint {
    /// Strategy family discriminator (mirrors [`StrategyConfig::kind`]).
    #[serde(rename = "type")]
    pub kind: Option<String>,

    /// Blank symbol hint for Turing machine families.
    pub blank: Option<usize>,
}

// ---- History and cycle-detection config ------------------------------------

/// Controls round-by-round history recording and attractor-cycle detection.
///
/// When `enabled` is `true` the engine stores per-match round logs.
/// Setting `include_cycle_metadata` additionally annotates the output with
/// detected attractor cycles.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct HistoryConfig {
    /// Whether to record full round-by-round match histories.
    pub enabled: bool,

    /// Whether to include attractor-cycle metadata in the output.
    #[serde(default)]
    pub include_cycle_metadata: bool,
}

/// Fully validated and defaulted tournament configuration, ready for the
/// engine to consume.  Produced by [`GamesConfig::normalize`] or
/// [`GamesConfig::normalize_with_root`].
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

/// Engine-level settings controlling execution mode, parallelism, GPU
/// acceleration, score aggregation, and complexity cost adjustments.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Interactive vs. batch execution mode.
    #[serde(default)]
    pub mode: EngineMode,

    /// Thread-level parallelism settings.
    #[serde(default)]
    pub parallelism: ParallelismConfig,

    /// Interval (ms) between TUI progress updates in interactive mode.
    #[serde(default = "default_progress_interval_ms")]
    pub progress_interval_ms: u64,

    /// Enable the fast-eval optimization for deterministic strategies.
    #[serde(default = "default_fast_eval")]
    pub fast_eval: bool,

    /// Hardware acceleration preference (CPU, Metal, or auto-detect).
    #[serde(default)]
    pub accelerator: AcceleratorMode,

    /// How per-match scores are combined into the leaderboard.
    #[serde(default)]
    pub score_aggregation: ScoreAggregation,

    /// Canonical-form algorithm for FSM grouping.
    #[serde(default)]
    pub fsm_grouping: FsmGroupingMode,

    /// Optional per-step complexity cost adjustments.
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

// ---- Accelerator, scoring, and grouping enums -----------------------------

/// Hardware acceleration mode for the tournament kernel.
///
/// `Auto` lets the engine choose; `Cpu` forces software-only execution;
/// `Metal` requires the GPU path (macOS only).
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AcceleratorMode {
    /// Automatically select CPU or Metal based on availability.
    #[default]
    Auto,

    /// Force CPU-only execution, disabling GPU acceleration.
    Cpu,

    /// Require Metal GPU acceleration (fails on non-macOS platforms).
    Metal,
}

impl AcceleratorMode {
    /// Returns `true` if this mode permits Metal acceleration.
    pub fn allows_metal(self) -> bool {
        !matches!(self, Self::Cpu)
    }

    /// Returns `true` if this mode *requires* Metal acceleration.
    pub fn requires_metal(self) -> bool {
        matches!(self, Self::Metal)
    }
}

/// How per-match scores are aggregated into the final leaderboard.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScoreAggregation {
    /// Average payoff across all rounds and repetitions.
    #[default]
    #[serde(alias = "average", alias = "avg")]
    Mean,

    /// Cumulative sum of payoffs across all rounds and repetitions.
    #[serde(alias = "sum")]
    Total,
}

/// Determines how finite-state machines are grouped for enumeration and
/// isomorphism checks.
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

/// Optional per-step complexity costs applied to strategy scores.
///
/// When `enabled`, the engine subtracts a small cost proportional to the
/// computational resources consumed by each strategy (TM steps, FSM states).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComplexityCostConfig {
    /// Whether complexity-cost adjustments are active.
    #[serde(default)]
    pub enabled: bool,

    /// Cost deducted per Turing machine execution step.
    #[serde(default)]
    pub tm_step_cost: f64,

    /// Cost deducted per FSM state in the strategy definition.
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

// ---- Engine execution mode and parallelism --------------------------------

/// Top-level execution mode for the tournament engine.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EngineMode {
    /// Interactive mode with live progress updates via the TUI.
    #[default]
    Interactive,

    /// Batch mode with no interactive output, suitable for CI / scripting.
    Batch,
}

/// Thread-level parallelism configuration for the tournament runner.
///
/// Can be either a named mode (`Auto`, `Off`) or an explicit thread count.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParallelismConfig {
    /// Named parallelism preset.
    Mode(ParallelismMode),

    /// Explicit thread count override.
    Threads {
        /// Number of worker threads to spawn.
        threads: usize,
    },
}

impl Default for ParallelismConfig {
    fn default() -> Self {
        Self::Mode(ParallelismMode::Auto)
    }
}

/// Named parallelism presets.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParallelismMode {
    /// Let the engine choose the thread count based on available cores.
    Auto,

    /// Run the tournament on a single thread (useful for deterministic replay).
    Off,
}

// ---- Serde default helpers ------------------------------------------------

/// Default progress reporting interval (milliseconds) for interactive mode.
pub(super) fn default_progress_interval_ms() -> u64 {
    80
}

/// Default for the fast-eval optimization flag.
pub(super) fn default_fast_eval() -> bool {
    true
}

/// Default for the save-data flag (persist match results to disk).
pub(super) fn default_save_data() -> bool {
    true
}

// ---- Normalized strategy specification ------------------------------------

/// A single strategy definition ready for tournament use, carrying its
/// identifier, optional display name, and the fully resolved kind-specific
/// parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategySpec {
    pub id: String,
    pub name: Option<String>,
    #[serde(flatten)]
    pub kind: StrategySpecKind,
}

/// Discriminated union of the strategy families supported by the engine.
///
/// Each variant holds the fully validated parameters for its family:
/// finite state machine, cellular automaton, or one-sided Turing machine.
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
