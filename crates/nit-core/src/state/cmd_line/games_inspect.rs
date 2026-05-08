//! Handlers for the larger `:games inspect`, `:games tm`, `:games ca`,
//! `:games analyze`, and `:games strategy[…]` arms of the command-line
//! dispatcher. Each entry point takes the full token slice and the
//! original trimmed input — the rule-tuple parsers in `family_run.rs`
//! re-scan the trimmed string for `{…}` blocks, so it must be passed
//! through verbatim.

use crate::state::{
    cmd_line::helpers::{normalize_path_token, strategy_id_prefers_fsm},
    family_run::{parse_ca_rule_tuple, parse_tm_input_token, parse_tm_rule_tuple},
    AppState, GamesAnalysisRequest,
};
use nit_games::analysis::AnalysisConfig;

const TM_INSPECT_DEFAULT_MAX_STEPS: u32 = 256;

pub(super) fn handle_games_inspect(state: &mut AppState, tokens: &[&str], trimmed: &str) {
    let rule_tuple = match parse_tm_rule_tuple(trimmed) {
        Ok(value) => value,
        Err(msg) => {
            state.status = Some(msg);
            return;
        }
    };

    // Three accepted shapes: `<strategy_id>`, `<fsm_index>`, or a tuple.
    // The third token is the operator's explicit target; if it parses as
    // a number it's an FSM index, otherwise it's a strategy id.
    let explicit_target = tokens
        .get(2)
        .copied()
        .filter(|token| !token.starts_with('{'));
    let explicit_fsm_index = explicit_target.and_then(parse_tm_input_token);
    let explicit_id = explicit_target.filter(|token| parse_tm_input_token(token).is_none());

    if rule_tuple.is_none() && explicit_id.is_none() && explicit_fsm_index.is_none() {
        state.status = Some(
            "Usage: :games inspect <strategy_id> | :games inspect <fsm_index> | :games inspect fsm {index,states,k} | :games inspect <strategy_id> {rule,states,symbols} | :games inspect {rule,states,symbols}"
                .into(),
        );
        return;
    }

    let mut spec: Option<nit_games::StrategySpec> = None;
    let mut definition: Option<nit_games::output::StrategyDefinition> = None;
    let mut source_label: Option<String> = None;

    if let Some(index) = explicit_fsm_index {
        let states = 2usize;
        let actions = 2usize;
        let (outputs, transitions) =
            match nit_games::strategy::decode_fsm_notebook_index(index, states, actions) {
                Ok(decoded) => decoded,
                Err(err) => {
                    state.status = Some(format!("FSM rule decode error: {err}"));
                    return;
                }
            };
        let def = nit_games::output::StrategyDefinition {
            id: format!("fsm_rule_{index}_{states}x{actions}"),
            name: Some(format!("FSM index {index} ({states}x{actions})")),
            kind: nit_games::config::StrategySpecKind::Fsm {
                num_states: states,
                start_state: 0,
                outputs,
                input_mode: Some(nit_games::strategy::InputMode::OpponentLastAction),
                transitions,
                index: Some(index),
            },
            rng_seed_a: None,
            rng_seed_b: None,
        };
        spec = Some(nit_games::StrategySpec {
            id: def.id.clone(),
            name: def.name.clone(),
            kind: def.kind.clone(),
        });
        definition = Some(def);
        source_label = Some("rule".into());
    }

    let tuple_prefers_fsm = rule_tuple.is_some()
        && explicit_id
            .map(|id| strategy_id_prefers_fsm(state, id))
            .unwrap_or(false);

    if spec.is_none() && tuple_prefers_fsm {
        let (index, states, actions) = rule_tuple.expect("checked is_some");
        let states = states as usize;
        let actions = actions as usize;
        let (outputs, transitions) =
            match nit_games::strategy::decode_fsm_notebook_index(index, states, actions) {
                Ok(decoded) => decoded,
                Err(err) => {
                    state.status = Some(format!("FSM rule decode error: {err}"));
                    return;
                }
            };
        let effective_id = explicit_id
            .map(str::to_string)
            .unwrap_or_else(|| format!("fsm_rule_{index}_{states}x{actions}"));
        let def = nit_games::output::StrategyDefinition {
            id: effective_id,
            name: Some(format!("FSM index {index} ({states}x{actions})")),
            kind: nit_games::config::StrategySpecKind::Fsm {
                num_states: states,
                start_state: 0,
                outputs,
                input_mode: Some(nit_games::strategy::InputMode::OpponentLastAction),
                transitions,
                index: Some(index),
            },
            rng_seed_a: None,
            rng_seed_b: None,
        };

        spec = Some(nit_games::StrategySpec {
            id: def.id.clone(),
            name: def.name.clone(),
            kind: def.kind.clone(),
        });
        definition = Some(def);
        source_label = Some("rule".into());
    }

    if spec.is_none() {
        if let Some((rule_code, states, symbols)) = rule_tuple {
            if states == 0 || symbols < 2 {
                state.status = Some(if states == 0 {
                    "TM rule tuple: states must be >= 1".into()
                } else {
                    "TM rule tuple: symbols must be >= 2".into()
                });
                return;
            }

            let (transitions, _remaining) = nit_games::strategy::decode_tm_rule_code_wolfram(
                rule_code,
                states as usize,
                symbols as usize,
            );
            let output_map: Vec<nit_games::game::Action> = (0..symbols)
                .map(|idx| {
                    if idx == 0 {
                        nit_games::game::Action::Cooperate
                    } else {
                        nit_games::game::Action::Defect
                    }
                })
                .collect();

            let effective_id = explicit_id
                .map(str::to_string)
                .unwrap_or_else(|| format!("tm_rule_{rule_code}_{states}x{symbols}"));
            let def = nit_games::output::StrategyDefinition {
                id: effective_id,
                name: Some(format!("Rule {rule_code} ({states}x{symbols})")),
                kind: nit_games::config::StrategySpecKind::OneSidedTm {
                    states,
                    symbols,
                    start_state: 1,
                    blank: 0,
                    fallback_symbol: Some(0),
                    max_steps_per_round: TM_INSPECT_DEFAULT_MAX_STEPS,
                    input_mode: nit_games::strategy::InputMode::OpponentLastAction,
                    output_map,
                    transitions,
                    rule_code: Some(rule_code),
                },
                rng_seed_a: None,
                rng_seed_b: None,
            };

            spec = Some(nit_games::StrategySpec {
                id: def.id.clone(),
                name: def.name.clone(),
                kind: def.kind.clone(),
            });
            definition = Some(def);
            source_label = Some("rule".into());
        }
    }

    if spec.is_none() {
        if let Some(run) = state.games.last_run.as_ref() {
            if let Some(def) = run
                .strategies
                .iter()
                .find(|s| s.id == explicit_id.unwrap_or_default())
                .cloned()
            {
                spec = Some(nit_games::StrategySpec {
                    id: def.id.clone(),
                    name: def.name.clone(),
                    kind: def.kind.clone(),
                });
                definition = Some(def);
                source_label = Some("run".into());
            }
        }
    }

    if spec.is_none() {
        let target_id = explicit_id.unwrap_or("tm_rule");
        let config_text = state.editor_buffer().content_as_string();
        match nit_games::config::GamesConfig::from_toml_with_root(
            &config_text,
            Some(&state.workspace_root),
        ) {
            Ok(config) => {
                if let Some(found) = config.strategies.iter().find(|s| s.id == target_id) {
                    spec = Some(found.clone());
                    definition = Some(nit_games::output::StrategyDefinition {
                        id: found.id.clone(),
                        name: found.name.clone(),
                        kind: found.kind.clone(),
                        rng_seed_a: None,
                        rng_seed_b: None,
                    });
                    source_label = Some("config".into());
                }
            }
            Err(err) => {
                close_other_games_popups(state, GamesPopup::StrategyInspect);
                state.games.strategy_inspect.last_error = Some(format!("Config error: {err}"));
                state.games.strategy_inspect.title = None;
                state.games.strategy_inspect.lines.clear();
                state.games.strategy_inspect.definition = None;
                state.games.strategy_inspect.selected_index = 0;
                state.games.strategy_inspect.scroll_offset = 0;
                state.games.strategy_inspect.definitions.clear();
                state.games.strategy_inspect.source_label = Some("config".into());
                state.status = Some("Games strategy inspect error".into());
                return;
            }
        }
    }

    let Some(spec) = spec else {
        let target_id = explicit_id.unwrap_or("tm_rule");
        close_other_games_popups(state, GamesPopup::StrategyInspect);
        state.games.strategy_inspect.last_error =
            Some(format!("Strategy '{target_id}' not found in run or config"));
        state.games.strategy_inspect.title = None;
        state.games.strategy_inspect.lines.clear();
        state.games.strategy_inspect.definition = None;
        state.games.strategy_inspect.selected_index = 0;
        state.games.strategy_inspect.scroll_offset = 0;
        state.games.strategy_inspect.definitions.clear();
        state.games.strategy_inspect.source_label = None;
        state.status = Some("Games strategy inspect error".into());
        return;
    };

    let intro = nit_games::introspect_strategy(&spec);
    let lines = nit_games::format_strategy_introspection(&intro);

    close_other_games_popups(state, GamesPopup::StrategyInspect);
    state.games.strategy_inspect.last_error = None;
    state.games.strategy_inspect.title = Some(format!("{} — inspect", spec.id));
    state.games.strategy_inspect.lines = lines;
    state.games.strategy_inspect.definition = definition;
    state.games.strategy_inspect.selected_index = 0;
    state.games.strategy_inspect.scroll_offset = 0;
    state.games.strategy_inspect.definitions.clear();
    state.games.strategy_inspect.source_label = source_label;
    state.status = Some(format!("Games inspect: {}", spec.id));
}

pub(super) fn handle_games_strategy_run(state: &mut AppState) {
    if state.games.last_run.is_none() {
        state.status = Some("No run loaded for strategy inspection".into());
        return;
    }
    let Some(run) = state.games.last_run.as_ref() else {
        return;
    };
    let definitions = run.strategies.clone();
    close_other_games_popups(state, GamesPopup::StrategyInspect);
    state.games.strategy_inspect.last_error = None;
    state.games.strategy_inspect.title = None;
    state.games.strategy_inspect.lines.clear();
    state.games.strategy_inspect.definition = None;
    state.games.strategy_inspect.selected_index = 0;
    state.games.strategy_inspect.scroll_offset = 0;
    state.games.strategy_inspect.definitions = definitions;
    state.games.strategy_inspect.source_label = Some("run".into());
    state.status = Some("Games strategy inspector opened".into());
}

pub(super) fn handle_games_strategy_config(state: &mut AppState) {
    let config_text = state.editor_buffer().content_as_string();
    match nit_games::config::GamesConfig::from_toml_with_root(
        &config_text,
        Some(&state.workspace_root),
    ) {
        Ok(config) => {
            close_other_games_popups(state, GamesPopup::StrategyInspect);
            state.games.strategy_inspect.last_error = None;
            state.games.strategy_inspect.title = None;
            state.games.strategy_inspect.lines.clear();
            state.games.strategy_inspect.definition = None;
            state.games.strategy_inspect.selected_index = 0;
            state.games.strategy_inspect.scroll_offset = 0;
            state.games.strategy_inspect.definitions = config
                .strategies
                .iter()
                .map(|spec| nit_games::output::StrategyDefinition {
                    id: spec.id.clone(),
                    name: spec.name.clone(),
                    kind: spec.kind.clone(),
                    rng_seed_a: None,
                    rng_seed_b: None,
                })
                .collect();
            state.games.strategy_inspect.source_label = Some("config".into());
            state.status = Some("Games strategy inspector opened".into());
        }
        Err(err) => {
            let msg = format!("Config error: {err}");
            close_other_games_popups(state, GamesPopup::StrategyInspect);
            state.games.strategy_inspect.last_error = Some(msg.clone());
            state.games.strategy_inspect.title = None;
            state.games.strategy_inspect.lines.clear();
            state.games.strategy_inspect.definition = None;
            state.games.strategy_inspect.selected_index = 0;
            state.games.strategy_inspect.scroll_offset = 0;
            state.games.strategy_inspect.definitions.clear();
            state.games.strategy_inspect.source_label = Some("config".into());
            state.status = Some(msg);
        }
    }
}

pub(super) fn handle_games_tm(state: &mut AppState, tokens: &[&str], trimmed: &str) {
    let mut idx = 2usize;
    let mut source = "config";
    if let Some(token) = tokens.get(idx) {
        if *token == "run" {
            source = "run";
            idx += 1;
        } else if *token == "config" {
            source = "config";
            idx += 1;
        }
    }

    let rule_tuple = match parse_tm_rule_tuple(trimmed) {
        Ok(value) => value,
        Err(msg) => {
            state.status = Some(msg.clone());
            close_other_games_popups(state, GamesPopup::TmSim);
            state.games.tm_sim.last_error = Some(msg);
            state.games.tm_sim.definition = None;
            state.games.tm_sim.input = None;
            state.games.tm_sim.steps_override = None;
            state.games.tm_sim.source_label = Some("rule".into());
            state.games.tm_sim.scroll_offset = 0;
            return;
        }
    };

    let mut numbers: Vec<u64> = Vec::new();
    let mut id: Option<String> = None;
    for token in tokens.iter().skip(idx) {
        if let Some(value) = parse_tm_input_token(token) {
            numbers.push(value);
            continue;
        }
        if id.is_none() {
            id = Some((*token).to_string());
        }
    }

    let Some(input) = numbers.first().copied() else {
        state.status = Some(
            "Usage: :games tm [run|config] <input> [steps] [strategy_id] | :games tm {rule_code, states, symbols} <input> [steps]"
                .into(),
        );
        return;
    };
    let steps_override = numbers.get(1).copied().and_then(|value| {
        if value > u32::MAX as u64 {
            None
        } else {
            Some(value as u32)
        }
    });

    if let Some((rule_code, states, symbols)) = rule_tuple {
        if states == 0 || symbols < 2 {
            let msg: String = if states == 0 {
                "TM rule tuple: states must be >= 1".into()
            } else {
                "TM rule tuple: symbols must be >= 2".into()
            };
            state.status = Some(msg.clone());
            close_other_games_popups(state, GamesPopup::TmSim);
            state.games.tm_sim.last_error = Some(msg);
            state.games.tm_sim.definition = None;
            state.games.tm_sim.input = Some(input);
            state.games.tm_sim.steps_override = steps_override;
            state.games.tm_sim.source_label = Some("rule".into());
            state.games.tm_sim.scroll_offset = 0;
            return;
        }
        let (transitions, _remaining) = nit_games::strategy::decode_tm_rule_code_wolfram(
            rule_code,
            states as usize,
            symbols as usize,
        );
        let output_map: Vec<nit_games::game::Action> = (0..symbols)
            .map(|idx| {
                if idx == 0 {
                    nit_games::game::Action::Cooperate
                } else {
                    nit_games::game::Action::Defect
                }
            })
            .collect();
        let max_steps = steps_override.unwrap_or(TM_INSPECT_DEFAULT_MAX_STEPS);
        let def = nit_games::output::StrategyDefinition {
            id: format!("tm_rule_{rule_code}_{states}x{symbols}"),
            name: Some(format!("Rule {rule_code} ({states}x{symbols})")),
            kind: nit_games::config::StrategySpecKind::OneSidedTm {
                states,
                symbols,
                start_state: 1,
                blank: 0,
                fallback_symbol: Some(0),
                max_steps_per_round: max_steps,
                input_mode: nit_games::strategy::InputMode::OpponentLastAction,
                output_map,
                transitions,
                rule_code: Some(rule_code),
            },
            rng_seed_a: None,
            rng_seed_b: None,
        };

        close_other_games_popups(state, GamesPopup::TmSim);
        state.games.tm_sim.last_error = None;
        state.games.tm_sim.definition = Some(def);
        state.games.tm_sim.input = Some(input);
        state.games.tm_sim.steps_override = steps_override;
        state.games.tm_sim.source_label = Some("rule".into());
        state.games.tm_sim.scroll_offset = 0;
        state.status = Some("TM simulation opened (rule tuple)".into());
        return;
    }

    let mut source_label = source.to_string();
    let defs: Vec<nit_games::output::StrategyDefinition>;
    match source {
        "run" => {
            if let Some(run) = state.games.last_run.as_ref() {
                defs = run.strategies.clone();
            } else {
                state.status = Some("No run loaded for TM simulation".into());
                return;
            }
        }
        _ => {
            let config_text = state.editor_buffer().content_as_string();
            match nit_games::config::GamesConfig::from_toml_with_root(
                &config_text,
                Some(&state.workspace_root),
            ) {
                Ok(config) => {
                    defs = config
                        .strategies
                        .iter()
                        .map(|spec| nit_games::output::StrategyDefinition {
                            id: spec.id.clone(),
                            name: spec.name.clone(),
                            kind: spec.kind.clone(),
                            rng_seed_a: None,
                            rng_seed_b: None,
                        })
                        .collect();
                    source_label = "config".into();
                }
                Err(err) => {
                    let msg = format!("Config error: {err}");
                    state.status = Some(msg.clone());
                    close_other_games_popups(state, GamesPopup::TmSim);
                    state.games.tm_sim.last_error = Some(msg);
                    state.games.tm_sim.definition = None;
                    state.games.tm_sim.input = Some(input);
                    state.games.tm_sim.steps_override = steps_override;
                    state.games.tm_sim.source_label = Some("config".into());
                    state.games.tm_sim.scroll_offset = 0;
                    return;
                }
            }
        }
    }

    let mut tm_defs: Vec<nit_games::output::StrategyDefinition> = defs
        .into_iter()
        .filter(|def| {
            matches!(
                def.kind,
                nit_games::config::StrategySpecKind::OneSidedTm { .. }
            )
        })
        .collect();

    let selected = if let Some(id) = id.as_ref() {
        tm_defs
            .iter()
            .position(|def| def.id == *id)
            .map(|idx| tm_defs.remove(idx))
    } else if tm_defs.len() == 1 {
        tm_defs.pop()
    } else {
        None
    };

    let Some(def) = selected else {
        if tm_defs.is_empty() {
            state.status = Some("No one-sided TM strategies found".into());
        } else {
            state.status = Some("Multiple TM strategies found; specify an id".into());
        }
        return;
    };

    close_other_games_popups(state, GamesPopup::TmSim);
    state.games.tm_sim.last_error = None;
    state.games.tm_sim.definition = Some(def);
    state.games.tm_sim.input = Some(input);
    state.games.tm_sim.steps_override = steps_override;
    state.games.tm_sim.source_label = Some(source_label);
    state.games.tm_sim.scroll_offset = 0;
    state.status = Some("TM simulation opened".into());
}

pub(super) fn handle_games_ca(state: &mut AppState, tokens: &[&str], trimmed: &str) {
    let mut idx = 2usize;
    let mut source = "config";
    if let Some(token) = tokens.get(idx) {
        if *token == "run" {
            source = "run";
            idx += 1;
        } else if *token == "config" {
            source = "config";
            idx += 1;
        }
    }

    let rule_tuple = match parse_ca_rule_tuple(trimmed) {
        Ok(value) => value,
        Err(msg) => {
            state.status = Some(msg.clone());
            close_other_games_popups(state, GamesPopup::CaSim);
            state.games.ca_sim.last_error = Some(msg);
            state.games.ca_sim.definition = None;
            state.games.ca_sim.input = None;
            state.games.ca_sim.steps_override = None;
            state.games.ca_sim.source_label = Some("rule".into());
            state.games.ca_sim.scroll_offset = 0;
            return;
        }
    };

    let mut numbers: Vec<u64> = Vec::new();
    let mut id: Option<String> = None;
    for token in tokens.iter().skip(idx) {
        if let Some(value) = parse_tm_input_token(token) {
            numbers.push(value);
            continue;
        }
        // Skip stray characters from the brace tuple — these were parsed
        // already by `parse_ca_rule_tuple` and shouldn't be misread as ids.
        if token.contains('{') || token.contains('}') || token.contains(',') {
            continue;
        }
        if id.is_none() {
            id = Some((*token).to_string());
        }
    }

    let Some(input) = numbers.first().copied() else {
        state.status = Some(
            "Usage: :games ca [run|config] <input> [steps] [strategy_id] | :games ca {n,k,r} <input> [steps]"
                .into(),
        );
        return;
    };
    let steps_override = numbers.get(1).copied().and_then(|value| {
        if value > u32::MAX as u64 {
            None
        } else {
            Some(value as u32)
        }
    });

    if let Some((n, k, two_r, t)) = rule_tuple {
        let def = nit_games::output::StrategyDefinition {
            id: format!("ca_rule_{n}_{k}_{two_r}_{t}"),
            name: Some(format!(
                "CA rule {n} (k={k}, r={}, t={t})",
                two_r as f32 / 2.0
            )),
            kind: nit_games::config::StrategySpecKind::Ca {
                n,
                k,
                r: two_r as f32 / 2.0,
                t,
            },
            rng_seed_a: None,
            rng_seed_b: None,
        };

        close_other_games_popups(state, GamesPopup::CaSim);
        state.games.ca_sim.last_error = None;
        state.games.ca_sim.definition = Some(def);
        state.games.ca_sim.input = Some(input);
        state.games.ca_sim.steps_override = steps_override;
        state.games.ca_sim.source_label = Some("rule".into());
        state.games.ca_sim.scroll_offset = 0;
        state.status = Some("CA simulation opened (rule tuple)".into());
        return;
    }

    let mut source_label = source.to_string();
    let defs: Vec<nit_games::output::StrategyDefinition>;
    match source {
        "run" => {
            if let Some(run) = state.games.last_run.as_ref() {
                defs = run.strategies.clone();
            } else {
                state.status = Some("No run loaded for CA simulation".into());
                return;
            }
        }
        _ => {
            let config_text = state.editor_buffer().content_as_string();
            match nit_games::config::GamesConfig::from_toml_with_root(
                &config_text,
                Some(&state.workspace_root),
            ) {
                Ok(config) => {
                    defs = config
                        .strategies
                        .iter()
                        .map(|spec| nit_games::output::StrategyDefinition {
                            id: spec.id.clone(),
                            name: spec.name.clone(),
                            kind: spec.kind.clone(),
                            rng_seed_a: None,
                            rng_seed_b: None,
                        })
                        .collect();
                    source_label = "config".into();
                }
                Err(err) => {
                    let msg = format!("Config error: {err}");
                    state.status = Some(msg.clone());
                    close_other_games_popups(state, GamesPopup::CaSim);
                    state.games.ca_sim.last_error = Some(msg);
                    state.games.ca_sim.definition = None;
                    state.games.ca_sim.input = Some(input);
                    state.games.ca_sim.steps_override = steps_override;
                    state.games.ca_sim.source_label = Some("config".into());
                    state.games.ca_sim.scroll_offset = 0;
                    return;
                }
            }
        }
    }

    let mut ca_defs: Vec<nit_games::output::StrategyDefinition> = defs
        .into_iter()
        .filter(|def| matches!(def.kind, nit_games::config::StrategySpecKind::Ca { .. }))
        .collect();

    let selected = if let Some(id) = id.as_ref() {
        ca_defs
            .iter()
            .position(|def| def.id == *id)
            .map(|idx| ca_defs.remove(idx))
    } else if ca_defs.len() == 1 {
        ca_defs.pop()
    } else {
        None
    };

    let Some(def) = selected else {
        if ca_defs.is_empty() {
            state.status = Some("No CA strategies found".into());
        } else {
            state.status = Some("Multiple CA strategies found; specify an id".into());
        }
        return;
    };

    close_other_games_popups(state, GamesPopup::CaSim);
    state.games.ca_sim.last_error = None;
    state.games.ca_sim.definition = Some(def);
    state.games.ca_sim.input = Some(input);
    state.games.ca_sim.steps_override = steps_override;
    state.games.ca_sim.source_label = Some(source_label);
    state.games.ca_sim.scroll_offset = 0;
    state.status = Some("CA simulation opened".into());
}

pub(super) fn handle_games_analyze(state: &mut AppState, trimmed: &str) {
    let defaults = AnalysisConfig::default();
    let mut tail_rounds = defaults.tail_rounds;
    let mut trajectory_samples = defaults.trajectory_samples;
    let mut path: Option<String> = None;

    for arg in trimmed.split_whitespace().skip(2) {
        if let Some((key, value)) = arg.split_once('=') {
            match key.to_ascii_lowercase().as_str() {
                "tail" | "tail_rounds" => {
                    if let Ok(parsed) = value.parse::<usize>() {
                        tail_rounds = parsed;
                    }
                }
                "samples" | "trajectory_samples" => {
                    if let Ok(parsed) = value.parse::<usize>() {
                        trajectory_samples = parsed;
                    }
                }
                "path" => {
                    if !value.is_empty() {
                        path = Some(normalize_path_token(value));
                    }
                }
                _ => {}
            }
        } else if path.is_none() {
            path = Some(normalize_path_token(arg));
        }
    }

    if let Some(candidate) = path.as_ref() {
        if candidate.trim().is_empty() {
            path = None;
        }
    }

    if path.is_none() && state.games.last_history_path.is_none() {
        state.status = Some("No history log available to analyze".into());
        return;
    }

    state.games.pending_analyze = Some(GamesAnalysisRequest {
        path,
        tail_rounds,
        trajectory_samples,
    });
    state.games.analysis.open = true;
    state.games.analysis.last_error = None;
    state.games.analysis.summary = None;
    state.games.analysis.preview = None;
    state.games.analysis.scroll_offset = 0;
    state.status = Some("Games analysis queued".into());
}

#[derive(Copy, Clone)]
enum GamesPopup {
    StrategyInspect,
    TmSim,
    CaSim,
}

/// Mutually-exclusive games popups: opening one closes the others. The
/// command-line dispatcher does this by hand at every entry point, so
/// keeping it in one helper avoids the "did I close the right popup"
/// drift between the inspect / tm / ca / analyze handlers.
fn close_other_games_popups(state: &mut AppState, keep: GamesPopup) {
    let g = &mut state.games;
    g.run_browser.open = false;
    g.replay.open = false;
    g.match_history.open = false;
    g.strategy_inspect.open = matches!(keep, GamesPopup::StrategyInspect);
    g.analysis.open = false;
    g.tm_sim.open = matches!(keep, GamesPopup::TmSim);
    g.ca_sim.open = matches!(keep, GamesPopup::CaSim);
}
