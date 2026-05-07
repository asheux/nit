use super::*;
use crate::{lab::AppKind, prompt::Prompt, rule_protocol::RuleMode};

pub(super) fn handle_command_line(state: &mut AppState, input: &str) -> bool {
    let trimmed = input.trim();
    let cmd = trimmed.trim_start_matches(':').trim().to_lowercase();
    if cmd.is_empty() {
        return false;
    }
    let normalized = cmd
        .split_whitespace()
        .map(|token| token.trim_matches(':'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let tokens: Vec<&str> = normalized;
    if let Some(target_lab) = lab_from_tokens(&tokens) {
        if target_lab != state.app_kind {
            state.status = Some(format!(
                "{} lab not active (current: {}). Use --lab {} to start.",
                target_lab.label(),
                state.app_kind.label(),
                target_lab
            ));
            return false;
        }
    }
    if is_help_command_tokens(&tokens) {
        state.show_help = true;
        state.help_scroll = 0;
        state.status = Some("Help opened".into());
        return false;
    }
    match tokens.as_slice() {
        ["substrate"] | ["sub"] | ["sig"] | ["signals"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Signals;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["claims"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Claims;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["assumptions"] | ["asm"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Assumptions;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["q"] | ["quit"] | ["exit"] => {
            if state.has_unsaved_editor_buffers() {
                state.prompt = Some(Prompt::ConfirmQuit);
                false
            } else {
                true
            }
        }
        ["tree"] | ["nittree"] | ["explore"] => {
            state.file_tree.open = !state.file_tree.open;
            if state.file_tree.open {
                state.file_tree.root = state.workspace_root.clone();
                state.focus = PaneId::Editor;
                state.mode = Mode::Normal;
                state.status = Some("NITTree opened".into());
            } else {
                state.status = Some("NITTree closed".into());
            }
            false
        }
        ["find"] | ["ff"] => {
            state
                .fuzzy_search
                .open(SearchMode::Files, state.workspace_root.clone());
            state.status = Some("Search: files".into());
            false
        }
        ["grep"] | ["rg"] | ["search"] => {
            state
                .fuzzy_search
                .open(SearchMode::Content, state.workspace_root.clone());
            state.status = Some("Search: content".into());
            false
        }
        ["close"] => {
            if state.fuzzy_search.open {
                state.fuzzy_search.close();
                state.status = Some("Search closed".into());
            }
            false
        }
        ["run"] => match state.app_kind {
            AppKind::Gol => {
                state.visualizer.pending_run = true;
                state.visualizer.pending_snapshot = true;
                state.status = Some("Petri dish queued".into());
                false
            }
            AppKind::Games => {
                state.games.pending_run_override = None;
                state.games.pending_family_run = None;
                state.games.family_building = false;
                state.games.pending_run = true;
                state.status = Some("Games tournament queued".into());
                false
            }
        },
        ["gol", "run"] | ["run", "gol"] | ["life", "run"] | ["gol", "start"] | ["run", "life"] => {
            state.visualizer.pending_run = true;
            state.visualizer.pending_snapshot = true;
            state.status = Some("Petri dish queued".into());
            false
        }
        _ if tokens.first() == Some(&"games")
            && tokens.get(1) == Some(&"run")
            && tokens.len() > 2 =>
        {
            let (force, family) = if tokens.get(2) == Some(&"force") {
                match tokens.get(3).copied() {
                    Some(family) => (true, family),
                    None => {
                        state.status = Some(
                            "Usage: :games run force <fsm|ca|tm> {params} (e.g. :games run force fsm {3, 2})"
                                .into(),
                        );
                        return false;
                    }
                }
            } else {
                (false, tokens[2])
            };

            if state.games.family_building {
                state.status = Some("Family run preparation already in progress".into());
                return false;
            }

            match build_family_run_override(state, family, trimmed, force) {
                Ok(request) => {
                    state.games.pending_run_override = None;
                    state.games.pending_run = false;
                    state.games.pending_family_run = Some(request);
                    state.games.family_building = true;
                    let mode = if force { "forced, " } else { "" };
                    state.status = Some(format!("Preparing family run ({mode}{family})..."));
                }
                Err(err) => {
                    state.games.pending_family_run = None;
                    state.games.family_building = false;
                    state.status = Some(err)
                }
            }
            false
        }
        ["games", "run"] | ["run", "games"] => {
            state.games.pending_run_override = None;
            state.games.pending_family_run = None;
            state.games.family_building = false;
            state.games.pending_run = true;
            state.status = Some("Games tournament queued".into());
            false
        }
        ["gol", "hide"] | ["hide", "gol"] => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
            false
        }
        ["gol", "show"] | ["show", "gol"] => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
            false
        }
        ["gol", "stop"] | ["life", "stop"] => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
            false
        }
        ["run", "stop"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
            false
        }
        ["games", "hide"] | ["hide", "games"] => {
            state.games.pending_hide = true;
            state.status = Some("Games tournament hiding".into());
            false
        }
        ["games", "show"] | ["show", "games"] => {
            state.games.pending_show = true;
            state.status = Some("Games tournament showing".into());
            false
        }
        ["games", "stop"] | ["stop", "games"] => {
            state.games.pending_close = true;
            state.status = Some("Games tournament closing".into());
            false
        }
        ["games", "status"] => {
            state.status = Some(format!("Games status: {:?}", state.games.status));
            false
        }
        ["games", "runs"] | ["games", "browse"] | ["games", "browser"] => {
            state.games.replay.open = false;
            state.games.match_history.open = false;
            state.games.run_browser.open = true;
            state.games.run_browser.loading = true;
            state.games.run_browser.last_error = None;
            state.games.run_browser.entries.clear();
            state.games.run_browser.selected = 0;
            state.games.run_browser.scroll_offset = 0;
            state.games.pending_run_browser = true;
            state.status = Some("Games run browser opened".into());
            false
        }
        ["games", "replay"] => {
            if state.games.last_run.is_none() {
                state.status = Some("No run loaded for replay".into());
            } else {
                state.games.run_browser.open = false;
                state.games.match_history.open = false;
                state.games.replay.open = true;
                state.games.replay.loading = false;
                state.games.replay.last_error = None;
                state.games.replay.selected_pair = None;
                state.games.replay.selected_index = 0;
                state.games.replay.title = None;
                state.games.replay.lines.clear();
                state.games.replay.scroll_offset = 0;
                state.games.replay.cycle = None;
                state.status = Some("Games replay opened".into());
            }
            false
        }
        ["games", "history"] | ["games", "hist"] | ["games", "plot"] | ["games", "plots"] => {
            open_games_history_popup(state);
            false
        }
        ["history"] | ["hist"] | ["plot"] | ["plots"] if state.app_kind == AppKind::Games => {
            open_games_history_popup(state);
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"inspect") => {
            let rule_tuple = match parse_tm_rule_tuple(trimmed) {
                Ok(value) => value,
                Err(msg) => {
                    state.status = Some(msg);
                    return false;
                }
            };

            // Allow either:
            // - :games inspect <strategy_id>
            // - :games inspect <fsm_index>                (defaults to {index,2,2})
            // - :games inspect <strategy_id> {rule_code, states, symbols}
            // - :games inspect {rule_code, states, symbols}
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
                return false;
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
                            return false;
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
                            return false;
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
                        return false;
                    }

                    let (transitions, _remaining) =
                        nit_games::strategy::decode_tm_rule_code_wolfram(
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

                    let max_steps = 256;
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
                            max_steps_per_round: max_steps,
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

            if let Some(run) = state.games.last_run.as_ref() {
                if spec.is_none() {
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
                        state.games.run_browser.open = false;
                        state.games.replay.open = false;
                        state.games.tm_sim.open = false;
                        state.games.ca_sim.open = false;
                        state.games.analysis.open = false;
                        state.games.strategy_inspect.open = true;
                        state.games.strategy_inspect.last_error =
                            Some(format!("Config error: {err}"));
                        state.games.strategy_inspect.title = None;
                        state.games.strategy_inspect.lines.clear();
                        state.games.strategy_inspect.definition = None;
                        state.games.strategy_inspect.selected_index = 0;
                        state.games.strategy_inspect.scroll_offset = 0;
                        state.games.strategy_inspect.definitions.clear();
                        state.games.strategy_inspect.source_label = Some("config".into());
                        state.status = Some("Games strategy inspect error".into());
                        return false;
                    }
                }
            }

            let Some(spec) = spec else {
                let target_id = explicit_id.unwrap_or("tm_rule");
                state.games.run_browser.open = false;
                state.games.replay.open = false;
                state.games.tm_sim.open = false;
                state.games.ca_sim.open = false;
                state.games.analysis.open = false;
                state.games.strategy_inspect.open = true;
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
                return false;
            };

            let intro = nit_games::introspect_strategy(&spec);
            let lines = nit_games::format_strategy_introspection(&intro);

            state.games.run_browser.open = false;
            state.games.replay.open = false;
            state.games.tm_sim.open = false;
            state.games.ca_sim.open = false;
            state.games.analysis.open = false;
            state.games.strategy_inspect.open = true;
            state.games.strategy_inspect.last_error = None;
            state.games.strategy_inspect.title = Some(format!("{} — inspect", spec.id));
            state.games.strategy_inspect.lines = lines;
            state.games.strategy_inspect.definition = definition;
            state.games.strategy_inspect.selected_index = 0;
            state.games.strategy_inspect.scroll_offset = 0;
            state.games.strategy_inspect.definitions.clear();
            state.games.strategy_inspect.source_label = source_label;
            state.status = Some(format!("Games inspect: {}", spec.id));
            false
        }
        ["games", "strategy"]
        | ["games", "strategies"]
        | ["games", "strategy", "run"]
        | ["games", "strategies", "run"] => {
            if state.games.last_run.is_none() {
                state.status = Some("No run loaded for strategy inspection".into());
            } else if let Some(run) = state.games.last_run.as_ref() {
                state.games.run_browser.open = false;
                state.games.replay.open = false;
                state.games.tm_sim.open = false;
                state.games.ca_sim.open = false;
                state.games.strategy_inspect.open = true;
                state.games.strategy_inspect.last_error = None;
                state.games.strategy_inspect.title = None;
                state.games.strategy_inspect.lines.clear();
                state.games.strategy_inspect.definition = None;
                state.games.strategy_inspect.selected_index = 0;
                state.games.strategy_inspect.scroll_offset = 0;
                state.games.strategy_inspect.definitions = run.strategies.clone();
                state.games.strategy_inspect.source_label = Some("run".into());
                state.status = Some("Games strategy inspector opened".into());
            }
            false
        }
        ["games", "strategy", "all"]
        | ["games", "strategies", "all"]
        | ["games", "strategy", "config"]
        | ["games", "strategies", "config"] => {
            let config_text = state.editor_buffer().content_as_string();
            match nit_games::config::GamesConfig::from_toml_with_root(
                &config_text,
                Some(&state.workspace_root),
            ) {
                Ok(config) => {
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.tm_sim.open = false;
                    state.games.ca_sim.open = false;
                    state.games.strategy_inspect.open = true;
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
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.tm_sim.open = false;
                    state.games.ca_sim.open = false;
                    state.games.strategy_inspect.open = true;
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
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"tm") => {
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
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.strategy_inspect.open = false;
                    state.games.analysis.open = false;
                    state.games.ca_sim.open = false;
                    state.games.tm_sim.open = true;
                    state.games.tm_sim.last_error = Some(msg);
                    state.games.tm_sim.definition = None;
                    state.games.tm_sim.input = None;
                    state.games.tm_sim.steps_override = None;
                    state.games.tm_sim.source_label = Some("rule".into());
                    state.games.tm_sim.scroll_offset = 0;
                    return false;
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
                return false;
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
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.strategy_inspect.open = false;
                    state.games.analysis.open = false;
                    state.games.ca_sim.open = false;
                    state.games.tm_sim.open = true;
                    state.games.tm_sim.last_error = Some(msg);
                    state.games.tm_sim.definition = None;
                    state.games.tm_sim.input = Some(input);
                    state.games.tm_sim.steps_override = steps_override;
                    state.games.tm_sim.source_label = Some("rule".into());
                    state.games.tm_sim.scroll_offset = 0;
                    return false;
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
                let max_steps = steps_override.unwrap_or(256);
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

                state.games.run_browser.open = false;
                state.games.replay.open = false;
                state.games.strategy_inspect.open = false;
                state.games.analysis.open = false;
                state.games.ca_sim.open = false;
                state.games.tm_sim.open = true;
                state.games.tm_sim.last_error = None;
                state.games.tm_sim.definition = Some(def);
                state.games.tm_sim.input = Some(input);
                state.games.tm_sim.steps_override = steps_override;
                state.games.tm_sim.source_label = Some("rule".into());
                state.games.tm_sim.scroll_offset = 0;
                state.status = Some("TM simulation opened (rule tuple)".into());
                return false;
            }

            let mut source_label = source.to_string();
            let defs: Vec<nit_games::output::StrategyDefinition>;
            match source {
                "run" => {
                    if let Some(run) = state.games.last_run.as_ref() {
                        defs = run.strategies.clone();
                    } else {
                        state.status = Some("No run loaded for TM simulation".into());
                        return false;
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
                            state.games.run_browser.open = false;
                            state.games.replay.open = false;
                            state.games.strategy_inspect.open = false;
                            state.games.analysis.open = false;
                            state.games.ca_sim.open = false;
                            state.games.tm_sim.open = true;
                            state.games.tm_sim.last_error = Some(msg);
                            state.games.tm_sim.definition = None;
                            state.games.tm_sim.input = Some(input);
                            state.games.tm_sim.steps_override = steps_override;
                            state.games.tm_sim.source_label = Some("config".into());
                            state.games.tm_sim.scroll_offset = 0;
                            return false;
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
                return false;
            };

            state.games.run_browser.open = false;
            state.games.replay.open = false;
            state.games.strategy_inspect.open = false;
            state.games.analysis.open = false;
            state.games.ca_sim.open = false;
            state.games.tm_sim.open = true;
            state.games.tm_sim.last_error = None;
            state.games.tm_sim.definition = Some(def);
            state.games.tm_sim.input = Some(input);
            state.games.tm_sim.steps_override = steps_override;
            state.games.tm_sim.source_label = Some(source_label);
            state.games.tm_sim.scroll_offset = 0;
            state.status = Some("TM simulation opened".into());
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"ca") => {
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
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.strategy_inspect.open = false;
                    state.games.analysis.open = false;
                    state.games.tm_sim.open = false;
                    state.games.ca_sim.open = true;
                    state.games.ca_sim.last_error = Some(msg);
                    state.games.ca_sim.definition = None;
                    state.games.ca_sim.input = None;
                    state.games.ca_sim.steps_override = None;
                    state.games.ca_sim.source_label = Some("rule".into());
                    state.games.ca_sim.scroll_offset = 0;
                    return false;
                }
            };

            let mut numbers: Vec<u64> = Vec::new();
            let mut id: Option<String> = None;
            for token in tokens.iter().skip(idx) {
                if let Some(value) = parse_tm_input_token(token) {
                    numbers.push(value);
                    continue;
                }
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
                return false;
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

                state.games.run_browser.open = false;
                state.games.replay.open = false;
                state.games.strategy_inspect.open = false;
                state.games.analysis.open = false;
                state.games.tm_sim.open = false;
                state.games.ca_sim.open = true;
                state.games.ca_sim.last_error = None;
                state.games.ca_sim.definition = Some(def);
                state.games.ca_sim.input = Some(input);
                state.games.ca_sim.steps_override = steps_override;
                state.games.ca_sim.source_label = Some("rule".into());
                state.games.ca_sim.scroll_offset = 0;
                state.status = Some("CA simulation opened (rule tuple)".into());
                return false;
            }

            let mut source_label = source.to_string();
            let defs: Vec<nit_games::output::StrategyDefinition>;
            match source {
                "run" => {
                    if let Some(run) = state.games.last_run.as_ref() {
                        defs = run.strategies.clone();
                    } else {
                        state.status = Some("No run loaded for CA simulation".into());
                        return false;
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
                            state.games.run_browser.open = false;
                            state.games.replay.open = false;
                            state.games.strategy_inspect.open = false;
                            state.games.analysis.open = false;
                            state.games.tm_sim.open = false;
                            state.games.ca_sim.open = true;
                            state.games.ca_sim.last_error = Some(msg);
                            state.games.ca_sim.definition = None;
                            state.games.ca_sim.input = Some(input);
                            state.games.ca_sim.steps_override = steps_override;
                            state.games.ca_sim.source_label = Some("config".into());
                            state.games.ca_sim.scroll_offset = 0;
                            return false;
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
                return false;
            };

            state.games.run_browser.open = false;
            state.games.replay.open = false;
            state.games.strategy_inspect.open = false;
            state.games.analysis.open = false;
            state.games.tm_sim.open = false;
            state.games.ca_sim.open = true;
            state.games.ca_sim.last_error = None;
            state.games.ca_sim.definition = Some(def);
            state.games.ca_sim.input = Some(input);
            state.games.ca_sim.steps_override = steps_override;
            state.games.ca_sim.source_label = Some(source_label);
            state.games.ca_sim.scroll_offset = 0;
            state.status = Some("CA simulation opened".into());
            false
        }
        _ if tokens.first() == Some(&"games")
            && matches!(tokens.get(1), Some(&"analyze") | Some(&"analyse")) =>
        {
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
            } else {
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
            false
        }
        ["games", "export"] => {
            state.games.pending_export = true;
            false
        }
        ["gol", "seed"] => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
            false
        }
        ["seed", "view"] if state.app_kind == AppKind::Gol => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
            false
        }
        ["gol", "encoder"] => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
            false
        }
        ["gol", "encoder", name] => {
            if let Some(id) = SeedEncoderId::from_str_name(name) {
                state.visualizer.seed_encoder = id;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Encoder: {}", id.label()));
            } else {
                state.status = Some(format!("Unknown encoder: {name}"));
            }
            false
        }
        ["seed", "encoder"] if state.app_kind == AppKind::Gol => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
            false
        }
        ["seed", "encoder", name] if state.app_kind == AppKind::Gol => {
            if let Some(id) = SeedEncoderId::from_str_name(name) {
                state.visualizer.seed_encoder = id;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Encoder: {}", id.label()));
            } else {
                state.status = Some(format!("Unknown encoder: {name}"));
            }
            false
        }
        _ if tokens.first() == Some(&"gol") && tokens.get(1) == Some(&"rule") => {
            if tokens.len() == 2 {
                log_rule_overview(state);
            } else {
                let selector = trimmed
                    .split_whitespace()
                    .skip(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                match state.rule_catalog.select(&selector) {
                    Ok(selected) => apply_rule_selection(state, selected, true),
                    Err(err) => {
                        state.status = Some(format!(
                            "Invalid GoL rule '{selector}': {err}. Try B3/S23 or 'conway'."
                        ));
                    }
                }
            }
            false
        }
        _ if tokens.first() == Some(&"gol") && tokens.get(1) == Some(&"rules") => {
            log_rule_list(state);
            false
        }
        ["petri", "hide"] | ["hide", "petri"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
            false
        }
        ["petri", "show"] | ["show", "petri"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
            false
        }
        other => {
            state.status = Some(format!("Unknown command: {}", other.join(" ")));
            false
        }
    }
}

pub(super) fn is_help_command_tokens(tokens: &[&str]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let mut saw_keyword = false;
    let mut saw_question = false;
    for token in tokens {
        match *token {
            "help" | "commands" => saw_keyword = true,
            "?" => saw_question = true,
            "-" | "/" | "|" | "–" | "—" => {}
            _ => return false,
        }
    }
    saw_keyword || saw_question
}

pub(super) fn apply_rule_selection(state: &mut AppState, selected: SelectedRule, persist: bool) {
    let label = selected.name_first_label();
    match state.set_gol_rule(selected, persist) {
        Ok(changed) => {
            if changed {
                let suffix = if state.visualizer.running {
                    " Restarting Petri Dish session."
                } else {
                    ""
                };
                state.status = Some(format!("GoL rule set to {label}.{suffix}"));
            } else {
                state.status = Some(format!("GoL rule unchanged: {label}."));
            }
        }
        Err(err) => {
            state.status = Some(format!("GoL rule set to {label} (save failed: {err})"));
        }
    }
}

pub(super) fn normalize_path_token(value: &str) -> String {
    let trimmed = value.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|v| v.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    unquoted.trim().to_string()
}

pub(super) fn strategy_id_prefers_fsm(state: &AppState, id: &str) -> bool {
    if let Some(run) = state.games.last_run.as_ref() {
        if let Some(def) = run.strategies.iter().find(|def| def.id == id) {
            return matches!(def.kind, nit_games::config::StrategySpecKind::Fsm { .. });
        }
    }

    let config_text = state.editor_buffer().content_as_string();
    if let Ok(config) = nit_games::config::GamesConfig::from_toml_with_root(
        &config_text,
        Some(&state.workspace_root),
    ) {
        if let Some(spec) = config.strategies.iter().find(|spec| spec.id == id) {
            return matches!(spec.kind, nit_games::config::StrategySpecKind::Fsm { .. });
        }
    }

    id.eq_ignore_ascii_case("fsm") || id.starts_with("fsm")
}

pub(super) fn lab_from_tokens(tokens: &[&str]) -> Option<AppKind> {
    tokens
        .first()
        .and_then(|token| lab_from_token(token))
        .or_else(|| tokens.get(1).and_then(|token| lab_from_token(token)))
}

pub(super) fn lab_from_token(token: &str) -> Option<AppKind> {
    match token {
        "gol" | "life" => Some(AppKind::Gol),
        "games" => Some(AppKind::Games),
        _ => None,
    }
}

pub(super) fn apply_protocol_selection(
    state: &mut AppState,
    mut mode: RuleMode,
    label: Option<String>,
) {
    mode.reset();
    state.visualizer.rule_mode = mode;
    state.visualizer.protocol_name = label;
    let rule_ref = state.visualizer.rule_mode.current_rule().clone();
    state.visualizer.rule = rule_ref.rule.to_string();
    let mut selected = SelectedRule::from_rule(rule_ref.rule);
    if let Some(named) = state.rule_catalog.find_by_rule(rule_ref.rule) {
        selected.id = Some(named.id.clone());
        selected.name = Some(named.name.clone());
    } else {
        selected.id = rule_ref.id;
        selected.name = rule_ref.name;
    }
    state.gol_rule_selected = selected;
    state.visualizer.pending_rule_change = true;
}

pub(super) fn log_rule_overview(state: &mut AppState) {
    state.receive_log(format!(
        "Current GoL rule: {}",
        state.gol_rule_selected.label()
    ));
    let builtins: Vec<String> = state
        .rule_catalog
        .builtins()
        .map(|rule| format!("  {} — {} ({})", rule.id, rule.name, rule.rule))
        .collect();
    if !builtins.is_empty() {
        state.receive_log("Built-in rules:".to_string());
        for line in builtins {
            state.receive_log(line);
        }
    }
}

pub(super) fn log_rule_list(state: &mut AppState) {
    state.receive_log(format!("GoL rules ({} total):", state.rule_catalog.len()));
    let lines: Vec<String> = state
        .rule_catalog
        .iter()
        .map(|rule| format!("  {} — {} ({})", rule.id, rule.name, rule.rule))
        .collect();
    for line in lines {
        state.receive_log(line);
    }
    state.rule_picker.open = true;
    state.rule_picker.query.clear();
    state.rule_picker.selected = state
        .rule_catalog
        .index_of_selected(&state.gol_rule_selected)
        .unwrap_or(0);
}
