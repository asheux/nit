use super::payoff::payoff_from_config;
use super::strategy_parse::{
    load_generated_strategies, normalize_ca_kind, normalize_fsm_kind, normalize_tm_kind,
};
use super::types::{
    default_save_data, ConfigError, FamilyRunBaseConfig, FamilyRunParseConfig,
    FamilyRunStrategyHint, GamesConfig, NormalizedConfig, ParallelismConfig, PayoffConfig,
    StrategyConfig, StrategySpec, StrategySpecKind,
};
use super::{
    canonical_game_name, is_tm_kind, normalize_kind_str, ConfigResult, CONFIG_SCHEMA_VERSION,
};
use crate::game::{Action, PayoffMatrix};
use crate::strategy::InputMode;

struct RawBaseFields {
    schema_version: Option<u32>,
    game: Option<String>,
    rounds: Option<u32>,
    repetitions: Option<u32>,
    self_play: Option<bool>,
    noise: Option<f32>,
    payoff: Option<PayoffConfig>,
}

struct ValidatedBase {
    schema_version: u32,
    game: String,
    rounds: u32,
    repetitions: u32,
    self_play: bool,
    noise: f32,
    payoff: PayoffMatrix,
}

impl RawBaseFields {
    fn validate(self, errors: &mut Vec<String>) -> ValidatedBase {
        let schema_version = self.schema_version.unwrap_or(CONFIG_SCHEMA_VERSION);
        let game = resolve_game_name(self.game, errors);
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

        let payoff = match self.payoff {
            Some(p) => payoff_from_config(p, errors),
            None => PayoffMatrix::default_pd(),
        };

        ValidatedBase {
            schema_version,
            game,
            rounds,
            repetitions,
            self_play,
            noise,
            payoff,
        }
    }
}

fn validate_engine(engine: &super::types::EngineConfig, errors: &mut Vec<String>) {
    if let ParallelismConfig::Threads { threads } = engine.parallelism {
        if threads == 0 {
            errors.push("engine.parallelism.threads must be > 0".to_string());
        }
    }
}

fn parse_toml<T: serde::de::DeserializeOwned>(src: &str) -> ConfigResult<T> {
    toml::from_str(src).map_err(|err| ConfigError {
        errors: vec![err.to_string()],
    })
}

impl GamesConfig {
    pub fn from_toml(src: &str) -> ConfigResult<NormalizedConfig> {
        parse_toml::<Self>(src)?.normalize_with_root(None)
    }

    pub fn from_toml_with_root(
        src: &str,
        base_dir: Option<&std::path::Path>,
    ) -> ConfigResult<NormalizedConfig> {
        parse_toml::<Self>(src)?.normalize_with_root(base_dir)
    }

    pub fn family_run_base_from_toml_with_root(
        src: &str,
        _base_dir: Option<&std::path::Path>,
    ) -> ConfigResult<FamilyRunBaseConfig> {
        parse_toml::<FamilyRunParseConfig>(src)?.normalize_family_run_base()
    }

    pub fn normalize(self) -> ConfigResult<NormalizedConfig> {
        self.normalize_with_root(None)
    }

    pub fn normalize_with_root(
        self,
        base_dir: Option<&std::path::Path>,
    ) -> ConfigResult<NormalizedConfig> {
        let mut errors = Vec::new();
        let base = self.raw_base_fields().validate(&mut errors);

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
        validate_engine(&engine, &mut errors);

        if !errors.is_empty() {
            return Err(ConfigError { errors });
        }

        Ok(NormalizedConfig {
            schema_version: base.schema_version,
            game: base.game,
            rounds: base.rounds,
            repetitions: base.repetitions,
            self_play: base.self_play,
            save_data: self.save_data.unwrap_or_else(default_save_data),
            seed: self.seed,
            noise: base.noise,
            payoff: base.payoff,
            strategies,
            event_log: self.event_log.unwrap_or_default(),
            history: self.history.unwrap_or_default(),
            engine,
            max_memory_n: 0,
            tm_filter_applied: false,
        })
    }

    fn raw_base_fields(&self) -> RawBaseFields {
        RawBaseFields {
            schema_version: self.schema_version,
            game: self.game.clone(),
            rounds: self.rounds,
            repetitions: self.repetitions,
            self_play: self.self_play,
            noise: self.noise,
            payoff: self.payoff.clone(),
        }
    }
}

impl FamilyRunBaseConfig {
    pub fn from_normalized(config: &NormalizedConfig) -> Self {
        let tm_blank_hint = config.strategies.iter().find_map(|spec| match &spec.kind {
            StrategySpecKind::OneSidedTm { blank, .. } => Some(*blank),
            _ => None,
        });
        Self {
            schema_version: config.schema_version,
            game: config.game.clone(),
            rounds: config.rounds,
            repetitions: config.repetitions,
            self_play: config.self_play,
            save_data: config.save_data,
            seed: config.seed,
            noise: config.noise,
            payoff: config.payoff,
            event_log: config.event_log.clone(),
            history: config.history.clone(),
            engine: config.engine.clone(),
            tm_blank_hint,
        }
    }

    pub fn into_normalized(self, strategies: Vec<StrategySpec>) -> NormalizedConfig {
        NormalizedConfig {
            schema_version: self.schema_version,
            game: self.game,
            rounds: self.rounds,
            repetitions: self.repetitions,
            self_play: self.self_play,
            save_data: self.save_data,
            seed: self.seed,
            noise: self.noise,
            payoff: self.payoff,
            strategies,
            event_log: self.event_log,
            history: self.history,
            engine: self.engine,
            max_memory_n: 0,
            tm_filter_applied: false,
        }
    }
}

impl FamilyRunParseConfig {
    fn normalize_family_run_base(self) -> ConfigResult<FamilyRunBaseConfig> {
        let mut errors = Vec::new();
        let base = RawBaseFields {
            schema_version: self.schema_version,
            game: self.game,
            rounds: self.rounds,
            repetitions: self.repetitions,
            self_play: self.self_play,
            noise: self.noise,
            payoff: self.payoff,
        }
        .validate(&mut errors);

        let engine = self.engine.unwrap_or_default();
        validate_engine(&engine, &mut errors);

        if !errors.is_empty() {
            return Err(ConfigError { errors });
        }

        Ok(FamilyRunBaseConfig {
            schema_version: base.schema_version,
            game: base.game,
            rounds: base.rounds,
            repetitions: base.repetitions,
            self_play: base.self_play,
            save_data: self.save_data.unwrap_or_else(default_save_data),
            seed: self.seed,
            noise: base.noise,
            payoff: base.payoff,
            event_log: self.event_log.unwrap_or_default(),
            history: self.history.unwrap_or_default(),
            engine,
            tm_blank_hint: self.strategy.iter().find_map(family_run_tm_blank_hint),
        })
    }
}

fn resolve_game_name(raw_name: Option<String>, errors: &mut Vec<String>) -> String {
    let input = raw_name.unwrap_or_else(|| "ipd".to_string());
    match canonical_game_name(&input) {
        Some(canonical) => canonical.to_string(),
        None => {
            errors.push(format!("unsupported game '{input}' (expected ipd)"));
            input
        }
    }
}

fn family_run_tm_blank_hint(raw: &FamilyRunStrategyHint) -> Option<u8> {
    let kind_raw = normalize_kind_str(raw.kind.as_deref());
    if let Some(kind) = kind_raw.as_deref() {
        if !is_tm_kind(kind) {
            return None;
        }
    } else if raw.blank.is_none() {
        return None;
    }
    raw.blank.map(|b| b as u8)
}

fn normalize_strategy(
    raw: StrategyConfig,
    base_dir: Option<&std::path::Path>,
) -> Result<Vec<StrategySpec>, Vec<String>> {
    let mut errors = Vec::new();
    let kind_raw = normalize_kind_str(raw.kind.as_deref());
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
        kind if is_tm_kind(kind) => normalize_tm_kind(&raw, &mut errors),
        other => {
            errors.push(format!(
                "strategy '{id}': unknown type '{other}' (expected fsm, ca, tm, or generated)"
            ));
            fallback_fsm_spec()
        }
    };

    if errors.is_empty() {
        Ok(vec![StrategySpec { id, name, kind }])
    } else {
        Err(errors)
    }
}

fn fallback_fsm_spec() -> StrategySpecKind {
    StrategySpecKind::Fsm {
        num_states: 1,
        start_state: 0,
        outputs: vec![Action::Cooperate],
        input_mode: Some(InputMode::OpponentLastAction),
        transitions: vec![vec![0, 0]],
        index: None,
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
    let has_fsm_markers = raw.index.is_some()
        || raw.num_states.is_some()
        || raw.input_index_base.is_some()
        || raw.outputs.is_some()
        || (raw.transitions.is_some() && !has_tm_markers);
    let has_generated_markers = raw.source.is_some() || raw.limit.is_some();

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
