use crate::events::EventLogConfig;
use crate::game::{Action, PayoffMatrix};
use crate::strategy::{
    decode_fsm_notebook_index, decode_tm_rule_code_wolfram, InputMode, StrategyKind, TmMove,
    TmTransition,
};
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
    pub seed: Option<u64>,
    pub noise: Option<f32>,
    pub payoff: Option<PayoffConfig>,
    #[serde(default)]
    pub strategy: Vec<StrategyConfig>,
    pub event_log: Option<EventLogConfig>,
    pub history: Option<HistoryConfig>,
    pub engine: Option<EngineConfig>,
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
    pub seed: Option<u64>,
    pub noise: f32,
    pub payoff: PayoffMatrix,
    pub strategies: Vec<StrategySpec>,
    pub event_log: EventLogConfig,
    pub history: HistoryConfig,
    pub engine: EngineConfig,
    #[serde(skip)]
    pub max_memory_n: usize,
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

fn default_progress_interval_ms() -> u64 {
    80
}

fn default_fast_eval() -> bool {
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

impl GamesConfig {
    pub fn from_toml(src: &str) -> Result<NormalizedConfig, ConfigError> {
        let raw: GamesConfig = toml::from_str(src).map_err(|err| ConfigError {
            errors: vec![err.to_string()],
        })?;
        raw.normalize_with_root(None)
    }

    pub fn from_toml_with_root(
        src: &str,
        base_dir: Option<&std::path::Path>,
    ) -> Result<NormalizedConfig, ConfigError> {
        let raw: GamesConfig = toml::from_str(src).map_err(|err| ConfigError {
            errors: vec![err.to_string()],
        })?;
        raw.normalize_with_root(base_dir)
    }

    pub fn normalize(self) -> Result<NormalizedConfig, ConfigError> {
        self.normalize_with_root(None)
    }

    pub fn normalize_with_root(
        self,
        base_dir: Option<&std::path::Path>,
    ) -> Result<NormalizedConfig, ConfigError> {
        let mut errors = Vec::new();

        let schema_version = self.schema_version.unwrap_or(1);
        let game = self.game.unwrap_or_else(|| "ipd".to_string());
        let game = match game.as_str() {
            "pd" | "prisoners_dilemma" => "ipd".to_string(),
            other => other.to_string(),
        };
        let rounds = self.rounds.unwrap_or(200);
        let repetitions = self.repetitions.unwrap_or(1);
        let self_play = self.self_play.unwrap_or(true);
        let noise = self.noise.unwrap_or(0.0).clamp(0.0, 1.0);

        if rounds == 0 {
            errors.push("rounds must be > 0".to_string());
        }
        if repetitions == 0 {
            errors.push("repetitions must be > 0".to_string());
        }
        if !matches!(game.as_str(), "ipd") {
            errors.push(format!("unsupported game '{game}' (expected ipd)"));
        }

        let payoff = match self.payoff {
            Some(p) => payoff_from_config(p, &mut errors),
            None => PayoffMatrix::default_pd(),
        };

        let mut strategies = Vec::new();
        if self.strategy.is_empty() {
            errors.push("at least one strategy is required".to_string());
        }

        for raw in self.strategy {
            match normalize_strategy(raw, base_dir) {
                Ok(specs) => strategies.extend(specs),
                Err(errs) => errors.extend(errs),
            }
        }

        let engine = self.engine.unwrap_or_default();
        if let ParallelismConfig::Threads { threads } = engine.parallelism {
            if threads == 0 {
                errors.push("engine.parallelism.threads must be > 0".to_string());
            }
        }

        if !errors.is_empty() {
            return Err(ConfigError { errors });
        }

        Ok(NormalizedConfig {
            schema_version,
            game,
            rounds,
            repetitions,
            self_play,
            seed: self.seed,
            noise,
            payoff,
            strategies,
            event_log: self.event_log.unwrap_or_default(),
            history: self.history.unwrap_or_default(),
            engine,
            max_memory_n: 0,
        })
    }
}

fn normalize_strategy(
    raw: StrategyConfig,
    base_dir: Option<&std::path::Path>,
) -> Result<Vec<StrategySpec>, Vec<String>> {
    let mut errors = Vec::new();
    let kind_raw = raw
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let resolved_kind = match kind_raw.as_deref() {
        Some("auto") | None => match infer_strategy_kind(&raw) {
            Ok(kind) => kind.to_string(),
            Err(err) => {
                errors.push(err);
                "fsm".to_string()
            }
        },
        Some(kind) => kind.to_string(),
    };
    let id = raw.id.clone();
    let name = raw.name.clone();

    let kind = match resolved_kind.as_str() {
        "generated" | "generate" | "strategies" | "strategy_file" | "strategy_source" => {
            return load_generated_strategies(&id, raw.source.as_deref(), raw.limit, base_dir);
        }
        "fsm" => normalize_fsm_kind(&raw, &mut errors),
        "ca" | "cellular_automaton" | "cellular-automaton" => normalize_ca_kind(&raw, &mut errors),
        "leftside_tm"
        | "left-side-tm"
        | "one_sided_tm"
        | "one-sided-tm"
        | "one_sided_tm_strategy"
        | "tm"
        | "onesidedtm" => normalize_tm_kind(&raw, &mut errors),
        other => {
            errors.push(format!(
                "strategy '{id}': unknown type '{other}' (expected fsm, ca, tm, or generated)"
            ));
            StrategySpecKind::Fsm {
                num_states: 1,
                start_state: 0,
                outputs: vec![Action::Cooperate],
                input_mode: Some(InputMode::OpponentLastAction),
                transitions: vec![vec![0, 0]],
                index: None,
            }
        }
    };

    if errors.is_empty() {
        Ok(vec![StrategySpec { id, name, kind }])
    } else {
        Err(errors)
    }
}

fn infer_strategy_kind(raw: &StrategyConfig) -> Result<&'static str, String> {
    let has_ca_markers =
        raw.n.is_some() || raw.r.is_some() || raw.t.is_some() || raw.steps.is_some();
    let has_tm_markers = raw.rule_code.is_some()
        || raw.symbols.is_some()
        || raw.blank.is_some()
        || raw.fallback_symbol.is_some()
        || raw.max_steps_per_round.is_some()
        || raw.output_map.is_some();
    let mut has_fsm_markers = raw.index.is_some()
        || raw.num_states.is_some()
        || raw.input_index_base.is_some()
        || raw.outputs.is_some();
    let has_generated_markers = raw.source.is_some() || raw.limit.is_some();

    if raw.transitions.is_some() && !has_tm_markers {
        has_fsm_markers = true;
    }
    if raw.k.is_some() && raw.index.is_some() {
        has_fsm_markers = true;
    }

    let families = has_ca_markers as u8 + has_tm_markers as u8 + has_fsm_markers as u8;
    if families > 1 {
        return Err(format!(
            "strategy '{}': type omitted/auto but fields match multiple families; set type explicitly",
            raw.id
        ));
    }
    if has_ca_markers {
        return Ok("ca");
    }
    if has_tm_markers {
        return Ok("tm");
    }
    if has_fsm_markers {
        return Ok("fsm");
    }
    if has_generated_markers {
        return Ok("generated");
    }
    Ok("fsm")
}

fn normalize_fsm_kind(raw: &StrategyConfig, errors: &mut Vec<String>) -> StrategySpecKind {
    let id = raw.id.as_str();
    let input_mode = parse_input_mode(id, raw.input_mode.as_deref(), errors);
    if let Some(mode) = input_mode {
        if !matches!(mode, InputMode::OpponentLastAction) {
            errors.push(format!(
                "strategy '{id}': FSM uses notebook semantics and only supports input_mode=opponent_last_action"
            ));
        }
    }

    if let Some(index) = raw.index {
        if raw.transitions.is_some() || raw.outputs.is_some() {
            errors.push(format!(
                "strategy '{id}': fsm index encoding cannot be combined with explicit outputs/transitions"
            ));
        }
        let mut actions = raw.k.unwrap_or(2);
        if actions != 2 {
            errors.push(format!(
                "strategy '{id}': notebook-compatible FSM gameplay currently supports k=2 only"
            ));
            actions = 2;
        }
        let states = raw.num_states.or(raw.states).unwrap_or(0);
        if states == 0 {
            errors.push(format!(
                "strategy '{id}': num_states (or states) must be > 0 for indexed FSMs"
            ));
            return StrategySpecKind::Fsm {
                num_states: 0,
                start_state: 0,
                outputs: Vec::new(),
                input_mode: Some(InputMode::OpponentLastAction),
                transitions: Vec::new(),
                index: Some(index),
            };
        }
        let (outputs, transitions) = match decode_fsm_notebook_index(index, states, actions) {
            Ok(decoded) => decoded,
            Err(err) => {
                errors.push(format!("strategy '{id}': {err}"));
                (Vec::new(), Vec::new())
            }
        };
        StrategySpecKind::Fsm {
            num_states: states,
            start_state: 0,
            outputs,
            input_mode: Some(InputMode::OpponentLastAction),
            transitions,
            index: Some(index),
        }
    } else {
        let outputs_raw = raw.outputs.clone().unwrap_or_default();
        let outputs = parse_actions(id, "outputs", outputs_raw, errors);
        let mut input_index_base = raw.input_index_base.unwrap_or(0);
        if input_index_base > 1 {
            errors.push(format!("strategy '{id}': input_index_base must be 0 or 1"));
            input_index_base = 0;
        }
        let num_states = raw.num_states.or(raw.states).unwrap_or(outputs.len());
        if num_states == 0 {
            errors.push(format!("strategy '{id}': num_states must be > 0"));
        }
        if !outputs.is_empty() && outputs.len() != num_states {
            errors.push(format!(
                "strategy '{id}': outputs length {} must match num_states {num_states}",
                outputs.len()
            ));
        }
        let transitions = parse_fsm_transitions(
            id,
            raw.transitions.clone(),
            num_states,
            input_index_base,
            errors,
        );
        let start_state_raw = raw.start_state.unwrap_or(0);
        let start_state =
            normalize_index(id, "start_state", start_state_raw, input_index_base, errors);
        if start_state >= num_states && num_states > 0 {
            errors.push(format!(
                "strategy '{id}': start_state {start_state} out of range"
            ));
        }
        if num_states > 0 {
            for (row_idx, row) in transitions.iter().enumerate() {
                if row.len() != 2 {
                    errors.push(format!(
                        "strategy '{id}': transitions row {row_idx} must have 2 entries"
                    ));
                    continue;
                }
                for (col_idx, &next) in row.iter().enumerate() {
                    if next >= num_states {
                        errors.push(format!(
                            "strategy '{id}': transitions[{row_idx}][{col_idx}] = {next} out of range"
                        ));
                    }
                }
            }
        }
        StrategySpecKind::Fsm {
            num_states,
            start_state,
            outputs,
            input_mode: Some(InputMode::OpponentLastAction),
            transitions,
            index: None,
        }
    }
}

fn normalize_ca_kind(raw: &StrategyConfig, errors: &mut Vec<String>) -> StrategySpecKind {
    let id = raw.id.as_str();
    let n = raw.n.unwrap_or(0) as u64;
    let k_raw = raw.k.unwrap_or(2);
    let k = k_raw.clamp(2, u8::MAX as usize) as u8;
    if k_raw < 2 {
        errors.push(format!("strategy '{id}': ca.k must be >= 2"));
    }
    if k_raw > u8::MAX as usize {
        errors.push(format!("strategy '{id}': ca.k must be <= {}", u8::MAX));
    }
    let r_raw = raw.r.unwrap_or(-1.0);
    let two_r = match parse_two_r(r_raw) {
        Some(value) => value,
        None => {
            errors.push(format!(
                "strategy '{id}': ca.r must satisfy r >= 0 and IntegerQ[2r]"
            ));
            0
        }
    };
    let t = raw.t.or(raw.steps).unwrap_or(10);
    if t == 0 {
        errors.push(format!("strategy '{id}': ca.t must be > 0"));
    }

    let neighborhood = two_r.saturating_add(1) as u32;
    if let Some(table_len) = checked_pow_u128(k as u128, neighborhood) {
        if table_len > 1_000_000 {
            errors.push(format!(
                "strategy '{id}': ca rule table too large ({table_len} entries), reduce k or r"
            ));
        }
    } else {
        errors.push(format!(
            "strategy '{id}': ca rule table size overflow for k={k} r={}",
            two_r as f32 / 2.0
        ));
    }

    StrategySpecKind::Ca {
        n,
        k,
        r: two_r as f32 / 2.0,
        t,
    }
}

fn normalize_tm_kind(raw: &StrategyConfig, errors: &mut Vec<String>) -> StrategySpecKind {
    let id = raw.id.as_str();
    let states = raw.states.unwrap_or(0);
    let symbols = raw.symbols.unwrap_or(0);
    if states == 0 {
        errors.push(format!("strategy '{id}': states must be > 0"));
    }
    if symbols == 0 {
        errors.push(format!("strategy '{id}': symbols must be > 0"));
    }
    if states > u16::MAX as usize {
        errors.push(format!("strategy '{id}': states must be <= {}", u16::MAX));
    }
    if symbols > u8::MAX as usize {
        errors.push(format!("strategy '{id}': symbols must be <= {}", u8::MAX));
    }
    let start_state_raw = raw.start_state.unwrap_or(1);
    let blank_raw = raw.blank.unwrap_or(0);
    let fallback_raw = raw.fallback_symbol.unwrap_or(blank_raw);
    let max_steps = raw.max_steps_per_round.unwrap_or(256);
    let parsed_mode = parse_input_mode(id, raw.input_mode.as_deref(), errors);
    if let Some(mode) = parsed_mode {
        if !matches!(mode, InputMode::OpponentLastAction) {
            errors.push(format!(
                "strategy '{id}': TM uses notebook semantics and ignores player perspective; use input_mode=opponent_last_action or omit it"
            ));
        }
    }
    let output_map_raw = raw
        .output_map
        .clone()
        .unwrap_or_else(|| vec!["C".to_string(), "D".to_string()]);
    let mut output_map = parse_actions(id, "output_map", output_map_raw, errors);

    if states > 0 && (start_state_raw == 0 || start_state_raw > states) {
        errors.push(format!(
            "strategy '{id}': start_state must be in 1..={states}"
        ));
    }
    if symbols > 0 && blank_raw >= symbols {
        errors.push(format!(
            "strategy '{id}': blank symbol {blank_raw} out of range (symbols={symbols})"
        ));
    }
    if symbols > 0 && fallback_raw >= symbols {
        errors.push(format!(
            "strategy '{id}': fallback_symbol {fallback_raw} out of range (symbols={symbols})"
        ));
    }
    if symbols > 0 && output_map.len() < symbols {
        errors.push(format!(
            "strategy '{id}': output_map length {} must be >= symbols {symbols}",
            output_map.len()
        ));
    }

    if symbols > 0 {
        let symbols_usize = symbols;
        let mut notebook_output_map = Vec::with_capacity(symbols_usize);
        for symbol in 0..symbols_usize {
            notebook_output_map.push(if symbol == 0 {
                Action::Cooperate
            } else {
                Action::Defect
            });
        }
        if output_map.len() >= symbols_usize
            && output_map[..symbols_usize] != notebook_output_map[..symbols_usize]
        {
            errors.push(format!(
                "strategy '{id}': output_map must map 0->C and all non-zero symbols->D to match notebook semantics"
            ));
        }
        output_map = notebook_output_map;
    }

    let mut transitions = Vec::new();
    let has_transitions = raw.transitions.is_some();
    let has_rule = raw.rule_code.is_some();
    if has_transitions && has_rule {
        errors.push(format!(
            "strategy '{id}': specify either transitions or rule_code, not both"
        ));
    }
    if let Some(value) = raw.transitions.clone() {
        transitions = parse_tm_transitions(id, value, states, symbols, blank_raw, errors);
    } else if let Some(rule_code) = raw.rule_code {
        transitions = decode_tm_rule_code(id, rule_code, states, symbols, errors);
    } else {
        errors.push(format!(
            "strategy '{id}': tm requires transitions or rule_code"
        ));
    }

    StrategySpecKind::OneSidedTm {
        states: states as u16,
        symbols: symbols as u8,
        start_state: start_state_raw as u16,
        blank: blank_raw as u8,
        fallback_symbol: Some(fallback_raw as u8),
        max_steps_per_round: max_steps,
        input_mode: InputMode::OpponentLastAction,
        output_map,
        transitions,
        rule_code: raw.rule_code,
    }
}

fn parse_actions(
    id: &str,
    field: &str,
    values: Vec<String>,
    errors: &mut Vec<String>,
) -> Vec<Action> {
    let mut out = Vec::new();
    for value in values {
        match Action::parse(&value) {
            Some(action) => out.push(action),
            None => errors.push(format!(
                "strategy '{id}': invalid action '{value}' in {field}"
            )),
        }
    }
    out
}

fn normalize_index(
    id: &str,
    field: &str,
    value: usize,
    input_index_base: u8,
    errors: &mut Vec<String>,
) -> usize {
    if input_index_base == 1 {
        if value == 0 {
            errors.push(format!(
                "strategy '{id}': {field} must be >= 1 when input_index_base = 1"
            ));
            0
        } else {
            value - 1
        }
    } else {
        value
    }
}

fn parse_input_mode(id: &str, raw: Option<&str>, errors: &mut Vec<String>) -> Option<InputMode> {
    let raw = raw?;
    let normalized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match normalized.as_str() {
        "opponentlastaction" | "opponent" | "opp" | "opplastaction" => {
            Some(InputMode::OpponentLastAction)
        }
        "selflastaction" | "self" | "selflast" => Some(InputMode::SelfLastAction),
        "jointlastaction" | "joint" | "jointlast" | "combinedlastaction" | "combined"
        | "combinedlast" => Some(InputMode::JointLastAction),
        _ => {
            errors.push(format!(
                "strategy '{id}': invalid input_mode '{raw}' (expected opponent_last_action, self_last_action, or joint_last_action)"
            ));
            None
        }
    }
}

fn parse_fsm_transitions(
    id: &str,
    raw: Option<toml::Value>,
    num_states: usize,
    input_index_base: u8,
    errors: &mut Vec<String>,
) -> Vec<Vec<usize>> {
    let Some(value) = raw else {
        errors.push(format!("strategy '{id}': transitions required for fsm"));
        return Vec::new();
    };

    let rows: Vec<Vec<usize>> = match value.try_into() {
        Ok(rows) => rows,
        Err(err) => {
            errors.push(format!("strategy '{id}': invalid transitions: {err}"));
            return Vec::new();
        }
    };

    if rows.is_empty() {
        errors.push(format!("strategy '{id}': transitions must not be empty"));
        return Vec::new();
    }
    if num_states > 0 && rows.len() != num_states {
        errors.push(format!(
            "strategy '{id}': transitions length {} must match num_states {}",
            rows.len(),
            num_states
        ));
    }

    let first_len = rows.first().map(|row| row.len()).unwrap_or(0);
    let has_state_index = first_len == 3;
    let expected_len = if has_state_index { 3 } else { 2 };
    if first_len != 2 && first_len != 3 {
        errors.push(format!(
            "strategy '{id}': transitions row 0 length {first_len} must be 2 or 3"
        ));
    }

    let mut transitions = Vec::with_capacity(rows.len());
    for (row_idx, row) in rows.iter().enumerate() {
        if row.len() != expected_len {
            errors.push(format!(
                "strategy '{id}': transitions row {row_idx} must have {expected_len} entries"
            ));
            continue;
        }
        let start = if has_state_index { 1 } else { 0 };
        if has_state_index {
            let expected = if input_index_base == 1 {
                row_idx + 1
            } else {
                row_idx
            };
            if row[0] != expected {
                errors.push(format!(
                    "strategy '{id}': transitions row {row_idx} begins with state {}, expected {expected}",
                    row[0]
                ));
            }
            let _ = normalize_index(
                id,
                &format!("transitions[{row_idx}][0]"),
                row[0],
                input_index_base,
                errors,
            );
        }

        let mut nexts = Vec::with_capacity(2);
        for (col_idx, &value) in row[start..].iter().enumerate() {
            let next = normalize_index(
                id,
                &format!("transitions[{row_idx}][{}]", col_idx + start),
                value,
                input_index_base,
                errors,
            );
            nexts.push(next);
        }
        transitions.push(nexts);
    }

    transitions
}

#[derive(Debug, Deserialize)]
struct TmTransitionRule {
    state: usize,
    read: usize,
    write: usize,
    #[serde(rename = "move")]
    move_dir: TmMove,
    next: usize,
}

fn parse_tm_transitions(
    id: &str,
    raw: toml::Value,
    states: usize,
    symbols: usize,
    blank: usize,
    errors: &mut Vec<String>,
) -> Vec<TmTransition> {
    let raw_clone = raw.clone();
    if let Ok(rules) = raw_clone.try_into::<Vec<TmTransitionRule>>() {
        let total = states.saturating_mul(symbols);
        let mut transitions = vec![
            TmTransition {
                write: blank as u8,
                move_dir: TmMove::Stay,
                next: 0,
            };
            total
        ];
        let mut seen = vec![false; total];
        for rule in rules {
            if rule.state == 0 || rule.state > states {
                errors.push(format!(
                    "strategy '{id}': tm transition state {} out of range (1..={states})",
                    rule.state
                ));
                continue;
            }
            if rule.read >= symbols {
                errors.push(format!(
                    "strategy '{id}': tm transition read {} out of range (symbols={symbols})",
                    rule.read
                ));
                continue;
            }
            if rule.write >= symbols {
                errors.push(format!(
                    "strategy '{id}': tm transition write {} out of range (symbols={symbols})",
                    rule.write
                ));
                continue;
            }
            if rule.next > states {
                errors.push(format!(
                    "strategy '{id}': tm transition next {} out of range (0..={states})",
                    rule.next
                ));
                continue;
            }
            let idx = (rule.state - 1) * symbols + rule.read;
            if let Some(slot) = seen.get_mut(idx) {
                if *slot {
                    errors.push(format!(
                        "strategy '{id}': duplicate tm transition for state {} read {}",
                        rule.state, rule.read
                    ));
                    continue;
                }
                *slot = true;
            }
            if let Some(entry) = transitions.get_mut(idx) {
                *entry = TmTransition {
                    write: rule.write as u8,
                    move_dir: rule.move_dir,
                    next: rule.next as u16,
                };
            }
        }
        if seen.iter().any(|seen| !*seen) {
            let missing = seen.iter().filter(|seen| !**seen).count();
            errors.push(format!(
                "strategy '{id}': tm transitions missing {missing} (state, read) pairs"
            ));
        }
        return transitions;
    }

    match parse_tm_table_transitions(&raw, states, symbols) {
        Ok(transitions) => transitions,
        Err(err) => {
            errors.push(format!("strategy '{id}': invalid tm transitions: {err}"));
            Vec::new()
        }
    }
}

fn parse_tm_table_transitions(
    raw: &toml::Value,
    states: usize,
    symbols: usize,
) -> Result<Vec<TmTransition>, String> {
    let rows = raw
        .as_array()
        .ok_or_else(|| "expected transitions to be an array".to_string())?;
    if rows.len() != states {
        return Err(format!(
            "transitions table must have {states} rows (one per state)"
        ));
    }
    let total = states.saturating_mul(symbols);
    let mut transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Stay,
            next: 0,
        };
        total
    ];
    for (state_idx, row_val) in rows.iter().enumerate() {
        let row = row_val
            .as_array()
            .ok_or_else(|| format!("transitions[{state_idx}] must be an array"))?;
        if row.len() != symbols {
            return Err(format!(
                "transitions[{state_idx}] must have {symbols} entries (one per symbol)"
            ));
        }
        for (read_idx, entry_val) in row.iter().enumerate() {
            let entry = entry_val.as_array().ok_or_else(|| {
                format!("transitions[{state_idx}][{read_idx}] must be [next, write, move]")
            })?;
            if entry.len() != 3 {
                return Err(format!(
                    "transitions[{state_idx}][{read_idx}] must be [next, write, move]"
                ));
            }
            let next = entry[0].as_integer().ok_or_else(|| {
                format!("transitions[{state_idx}][{read_idx}][0] must be an integer")
            })?;
            let write = entry[1].as_integer().ok_or_else(|| {
                format!("transitions[{state_idx}][{read_idx}][1] must be an integer")
            })?;
            let move_dir = if let Some(move_int) = entry[2].as_integer() {
                match move_int {
                    -1 => TmMove::Left,
                    1 => TmMove::Right,
                    0 => TmMove::Stay,
                    other => {
                        return Err(format!(
                            "transitions[{state_idx}][{read_idx}][2] invalid move {other} (expected -1, 0, or 1)"
                        ))
                    }
                }
            } else if let Some(move_str) = entry[2].as_str() {
                let move_raw = move_str.trim().to_ascii_lowercase();
                match move_raw.as_str() {
                    "l" | "left" => TmMove::Left,
                    "r" | "right" => TmMove::Right,
                    "s" | "stay" => TmMove::Stay,
                    _ => {
                        return Err(format!(
                            "transitions[{state_idx}][{read_idx}][2] invalid move '{move_raw}'"
                        ))
                    }
                }
            } else {
                return Err(format!(
                    "transitions[{state_idx}][{read_idx}][2] must be a move string or integer"
                ));
            };
            if next < 0 || next as usize > states {
                return Err(format!(
                    "transitions[{state_idx}][{read_idx}][0] next {next} out of range (0..={states})"
                ));
            }
            if write < 0 || write as usize >= symbols {
                return Err(format!(
                    "transitions[{state_idx}][{read_idx}][1] write {write} out of range (symbols={symbols})"
                ));
            }
            let idx = state_idx * symbols + read_idx;
            transitions[idx] = TmTransition {
                write: write as u8,
                move_dir,
                next: next as u16,
            };
        }
    }
    if transitions.len() != total {
        return Err(format!(
            "transitions table size mismatch: expected {total} entries"
        ));
    }
    if states == 0 || symbols == 0 {
        return Err("transitions table requires states > 0 and symbols > 0".to_string());
    }
    Ok(transitions)
}

fn decode_tm_rule_code(
    id: &str,
    rule_code: u64,
    states: usize,
    symbols: usize,
    errors: &mut Vec<String>,
) -> Vec<TmTransition> {
    let (transitions, remaining) = decode_tm_rule_code_wolfram(rule_code, states, symbols);
    if states > 0 && symbols > 0 && remaining != 0 {
        errors.push(format!(
            "strategy '{id}': rule_code has unused higher digits for states={states} symbols={symbols}"
        ));
    }
    transitions
}

fn load_generated_strategies(
    id: &str,
    source: Option<&str>,
    limit: Option<usize>,
    base_dir: Option<&std::path::Path>,
) -> Result<Vec<StrategySpec>, Vec<String>> {
    let mut errors = Vec::new();
    let source = match source {
        Some(path) if !path.trim().is_empty() => path.trim(),
        _ => {
            errors.push(format!(
                "strategy '{id}': generated strategies require a source path"
            ));
            return Err(errors);
        }
    };

    let mut path = std::path::PathBuf::from(source);
    if path.is_relative() {
        if let Some(base) = base_dir {
            path = base.join(path);
        } else if let Ok(cwd) = std::env::current_dir() {
            path = cwd.join(path);
        }
    }

    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(err) => {
            errors.push(format!(
                "strategy '{id}': failed to open generated strategies {}: {err}",
                path.display()
            ));
            return Err(errors);
        }
    };
    use std::io::BufRead;
    let reader = std::io::BufReader::new(file);
    let mut specs = Vec::new();
    for (line_idx, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                errors.push(format!(
                    "strategy '{id}': failed reading generated strategies {}: {err}",
                    path.display()
                ));
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<StrategySpec>(trimmed) {
            Ok(mut spec) => {
                if !id.is_empty() {
                    spec.id = format!("{id}::{}", spec.id);
                }
                specs.push(spec);
                if let Some(limit) = limit {
                    if specs.len() >= limit {
                        break;
                    }
                }
            }
            Err(err) => {
                errors.push(format!(
                    "strategy '{id}': failed to parse generated strategies at line {}: {err}",
                    line_idx + 1
                ));
                break;
            }
        }
    }

    if errors.is_empty() {
        Ok(specs)
    } else {
        Err(errors)
    }
}

fn parse_two_r(r: f32) -> Option<u32> {
    if !r.is_finite() || r < 0.0 {
        return None;
    }
    let doubled = r * 2.0;
    let rounded = doubled.round();
    if (doubled - rounded).abs() > 1e-6 {
        return None;
    }
    if rounded < 0.0 {
        None
    } else {
        Some(rounded as u32)
    }
}

fn checked_pow_u128(base: u128, exp: u32) -> Option<u128> {
    let mut value: u128 = 1;
    for _ in 0..exp {
        value = value.checked_mul(base)?;
    }
    Some(value)
}

fn payoff_from_config(config: PayoffConfig, errors: &mut Vec<String>) -> PayoffMatrix {
    if let Some(matrix) = config.matrix.as_ref() {
        if matrix.len() != 2 {
            errors.push("payoff.matrix must have 2 rows".into());
            return fallback_payoff(config);
        }
        let mut cells = [[(0i32, 0i32); 2]; 2];
        for (row_idx, row) in matrix.iter().enumerate() {
            if row.len() != 2 {
                errors.push(format!("payoff.matrix row {row_idx} must have 2 columns"));
                return fallback_payoff(config);
            }
            for (col_idx, cell) in row.iter().enumerate() {
                if cell.len() != 2 {
                    errors.push(format!(
                        "payoff.matrix cell [{row_idx}][{col_idx}] must have 2 entries"
                    ));
                    return fallback_payoff(config);
                }
                cells[row_idx][col_idx] = (cell[0], cell[1]);
            }
        }
        let r = cells[0][0].0;
        let s = cells[0][1].0;
        let t = cells[1][0].0;
        let p = cells[1][1].0;
        let symmetric =
            cells[0][0].1 == r && cells[0][1].1 == t && cells[1][0].1 == s && cells[1][1].1 == p;
        if symmetric {
            if let Some(value) = config.r {
                if value != r {
                    errors.push("payoff.R does not match payoff.matrix[0][0]".into());
                }
            }
            if let Some(value) = config.s {
                if value != s {
                    errors.push("payoff.S does not match payoff.matrix[0][1]".into());
                }
            }
            if let Some(value) = config.t {
                if value != t {
                    errors.push("payoff.T does not match payoff.matrix[1][0]".into());
                }
            }
            if let Some(value) = config.p {
                if value != p {
                    errors.push("payoff.P does not match payoff.matrix[1][1]".into());
                }
            }
        }
        let matrix = [
            [
                [cells[0][0].0, cells[0][0].1],
                [cells[0][1].0, cells[0][1].1],
            ],
            [
                [cells[1][0].0, cells[1][0].1],
                [cells[1][1].0, cells[1][1].1],
            ],
        ];
        PayoffMatrix::from_matrix(matrix)
    } else {
        fallback_payoff(config)
    }
}

fn fallback_payoff(config: PayoffConfig) -> PayoffMatrix {
    let r = config.r.unwrap_or(3);
    let s = config.s.unwrap_or(0);
    let t = config.t.unwrap_or(5);
    let p = config.p.unwrap_or(1);
    let matrix = [[[r, r], [s, t]], [[t, s], [p, p]]];
    PayoffMatrix::from_matrix(matrix)
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
