use crate::events::EventLogConfig;
use crate::game::{Action, PayoffMatrix};
use crate::strategy::StrategyKind;
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
    pub kind: String,
    pub name: Option<String>,
    pub builtin: Option<String>,
    pub p_cooperate: Option<f32>,
    pub start_state: Option<usize>,
    pub input_index_base: Option<u8>,
    pub output: Option<Vec<String>>,
    pub transitions: Option<Vec<Vec<usize>>>,
    pub n: Option<usize>,
    pub table: Option<Vec<String>>,
    pub initial: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryConfig {
    pub enabled: bool,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
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
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            mode: EngineMode::Interactive,
            parallelism: ParallelismConfig::default(),
            progress_interval_ms: default_progress_interval_ms(),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EngineMode {
    Interactive,
    Batch,
}

impl Default for EngineMode {
    fn default() -> Self {
        EngineMode::Interactive
    }
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

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinKind {
    AllC,
    AllD,
    TitForTat,
    GrimTrigger,
    WinStayLoseShift,
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
    Builtin {
        builtin: BuiltinKind,
    },
    Random {
        p_cooperate: f32,
    },
    Fsm {
        start_state: usize,
        #[serde(default)]
        input_index_base: u8,
        output: Vec<Action>,
        transitions: Vec<[usize; 4]>,
    },
    Memory {
        n: usize,
        initial: Action,
        table: Vec<Action>,
    },
}

impl GamesConfig {
    pub fn from_toml(src: &str) -> Result<NormalizedConfig, ConfigError> {
        let raw: GamesConfig = toml::from_str(src).map_err(|err| ConfigError {
            errors: vec![err.to_string()],
        })?;
        raw.normalize()
    }

    pub fn normalize(self) -> Result<NormalizedConfig, ConfigError> {
        let mut errors = Vec::new();

        let schema_version = self.schema_version.unwrap_or(1);
        let game = self.game.unwrap_or_else(|| "ipd".to_string());
        let game = match game.as_str() {
            "pd" | "prisoners_dilemma" => "ipd".to_string(),
            other => other.to_string(),
        };
        let rounds = self.rounds.unwrap_or(200);
        let repetitions = self.repetitions.unwrap_or(1);
        let self_play = self.self_play.unwrap_or(false);
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
        let mut max_memory_n = 0usize;

        if self.strategy.is_empty() {
            errors.push("at least one strategy is required".to_string());
        }

        for raw in self.strategy {
            match normalize_strategy(raw) {
                Ok(spec) => {
                    if let StrategySpecKind::Memory { n, .. } = spec.kind {
                        max_memory_n = max_memory_n.max(n);
                    }
                    strategies.push(spec);
                }
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
            max_memory_n,
        })
    }
}

fn normalize_strategy(raw: StrategyConfig) -> Result<StrategySpec, Vec<String>> {
    let mut errors = Vec::new();
    let kind_raw = raw.kind.trim().to_ascii_lowercase();
    let id = raw.id.clone();
    let name = raw.name.clone();

    let kind = match kind_raw.as_str() {
        "builtin" => {
            let builtin_key = raw
                .builtin
                .clone()
                .or_else(|| Some(raw.id.clone()))
                .or(raw.name.clone())
                .unwrap_or_default();
            match parse_builtin(&builtin_key) {
                Some(builtin) => StrategySpecKind::Builtin { builtin },
                None => {
                    errors.push(format!("strategy '{id}': unknown builtin '{builtin_key}'"));
                    StrategySpecKind::Builtin {
                        builtin: BuiltinKind::AllC,
                    }
                }
            }
        }
        "random" | "rand" => {
            let p = raw.p_cooperate.unwrap_or(0.5);
            if !(0.0..=1.0).contains(&p) {
                errors.push(format!("strategy '{id}': p_cooperate must be in [0,1]"));
            }
            StrategySpecKind::Random { p_cooperate: p }
        }
        "fsm" => {
            let output = raw.output.unwrap_or_default();
            let outputs = parse_actions(&id, "output", output, &mut errors);
            let mut input_index_base = raw.input_index_base.unwrap_or(0);
            if input_index_base > 1 {
                errors.push(format!("strategy '{id}': input_index_base must be 0 or 1"));
                input_index_base = 0;
            }
            let transitions = parse_transitions(
                &id,
                raw.transitions.unwrap_or_default(),
                input_index_base,
                &mut errors,
            );
            let start_state_raw = raw.start_state.unwrap_or(0);
            let start_state = if input_index_base == 1 {
                if start_state_raw == 0 {
                    errors.push(format!(
                        "strategy '{id}': start_state must be >= 1 when input_index_base = 1"
                    ));
                    0
                } else {
                    start_state_raw - 1
                }
            } else {
                start_state_raw
            };
            if start_state >= outputs.len() {
                errors.push(format!(
                    "strategy '{id}': start_state {start_state} out of range"
                ));
            }
            if !outputs.is_empty() {
                for (row_idx, row) in transitions.iter().enumerate() {
                    for (col_idx, &next) in row.iter().enumerate() {
                        if next >= outputs.len() {
                            errors.push(format!(
                                "strategy '{id}': transitions[{row_idx}][{col_idx}] = {next} out of range"
                            ));
                        }
                    }
                }
            }
            StrategySpecKind::Fsm {
                start_state,
                input_index_base,
                output: outputs,
                transitions,
            }
        }
        "memory" | "memory_n" | "memory-n" => {
            let n = raw.n.unwrap_or(1);
            if n == 0 || n > 10 {
                errors.push(format!("strategy '{id}': n must be 1..=10"));
            }
            let initial = raw
                .initial
                .as_deref()
                .and_then(Action::from_str)
                .unwrap_or(Action::Cooperate);
            let table = parse_actions(&id, "table", raw.table.unwrap_or_default(), &mut errors);
            let expected = 4usize.pow(n as u32);
            if table.len() != expected {
                errors.push(format!(
                    "strategy '{id}': table size {} does not match 4^n = {expected}",
                    table.len()
                ));
            }
            StrategySpecKind::Memory { n, initial, table }
        }
        other => {
            if let Some(builtin) = parse_builtin(other) {
                StrategySpecKind::Builtin { builtin }
            } else {
                errors.push(format!("strategy '{id}': unknown type '{other}'"));
                StrategySpecKind::Builtin {
                    builtin: BuiltinKind::AllC,
                }
            }
        }
    };

    if matches!(kind, StrategySpecKind::Fsm { .. }) {
        if let StrategySpecKind::Fsm {
            ref output,
            ref transitions,
            ..
        } = kind
        {
            if output.is_empty() {
                errors.push(format!("strategy '{id}': output must not be empty"));
            }
            if transitions.len() != output.len() {
                errors.push(format!(
                    "strategy '{id}': transitions length {} must match output length {}",
                    transitions.len(),
                    output.len()
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(StrategySpec { id, name, kind })
    } else {
        Err(errors)
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
        match Action::from_str(&value) {
            Some(action) => out.push(action),
            None => errors.push(format!(
                "strategy '{id}': invalid action '{value}' in {field}"
            )),
        }
    }
    out
}

fn parse_transitions(
    id: &str,
    rows: Vec<Vec<usize>>,
    input_index_base: u8,
    errors: &mut Vec<String>,
) -> Vec<[usize; 4]> {
    let mut out = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        if row.len() != 4 {
            errors.push(format!(
                "strategy '{id}': transitions row {idx} must have 4 entries"
            ));
            continue;
        }
        let mut arr = [0usize; 4];
        for (i, v) in row.iter().enumerate() {
            if input_index_base == 1 {
                if *v == 0 {
                    errors.push(format!(
                        "strategy '{id}': transitions[{idx}][{i}] must be >= 1 when input_index_base = 1"
                    ));
                    arr[i] = 0;
                } else {
                    arr[i] = *v - 1;
                }
            } else {
                arr[i] = *v;
            }
        }
        out.push(arr);
    }
    out
}

fn parse_builtin(input: &str) -> Option<BuiltinKind> {
    let normalized: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match normalized.as_str() {
        "allc" | "alwayscooperate" => Some(BuiltinKind::AllC),
        "alld" | "alwaysdefect" => Some(BuiltinKind::AllD),
        "tft" | "titfortat" => Some(BuiltinKind::TitForTat),
        "grim" | "grimtrigger" => Some(BuiltinKind::GrimTrigger),
        "wsls" | "winstayloseshift" | "pavlov" => Some(BuiltinKind::WinStayLoseShift),
        _ => None,
    }
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
            StrategySpecKind::Builtin { .. } => StrategyKind::Builtin,
            StrategySpecKind::Random { .. } => StrategyKind::Random,
            StrategySpecKind::Fsm { .. } => StrategyKind::Fsm,
            StrategySpecKind::Memory { .. } => StrategyKind::Memory,
        }
    }
}
