//! Lookup, parsing, and side-effect helpers shared by the command-line
//! dispatcher in `cmd_line.rs`. These were inline in the dispatcher; pulling
//! them out keeps the dispatcher focused on routing.

use crate::{gol_rules::SelectedRule, lab::AppKind, rule_protocol::RuleMode, state::AppState};

/// True if `tokens` decodes to one of the recognised "help" forms:
/// `help`, `commands`, `?`, or any combination separated by `-` / `/` / `|`
/// (and their unicode dash variants).
pub(crate) fn is_help_command_tokens(tokens: &[&str]) -> bool {
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

/// Apply a `SelectedRule` from the picker / `:gol rule <selector>` and
/// emit a status message that reflects whether the rule actually changed
/// and whether the petri dish is mid-run.
pub(crate) fn apply_rule_selection(state: &mut AppState, selected: SelectedRule, persist: bool) {
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

pub(crate) fn normalize_path_token(value: &str) -> String {
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

/// True if `id` resolves to an FSM strategy in the current run or workspace
/// config, OR if its name follows the `fsm*` convention. Used by the games
/// inspect dispatcher to disambiguate `<id> {tuple}` between FSM and TM.
pub(crate) fn strategy_id_prefers_fsm(state: &AppState, id: &str) -> bool {
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

pub(crate) fn lab_from_tokens(tokens: &[&str]) -> Option<AppKind> {
    tokens
        .first()
        .and_then(|token| lab_from_token(token))
        .or_else(|| tokens.get(1).and_then(|token| lab_from_token(token)))
}

pub(crate) fn lab_from_token(token: &str) -> Option<AppKind> {
    match token {
        "gol" | "life" => Some(AppKind::Gol),
        "games" => Some(AppKind::Games),
        _ => None,
    }
}

/// Resolve `mode` and label, push them onto the visualizer state, and
/// reseed the rule picker / current rule from the resolved `RuleRef`.
/// Mutates `state.gol_rule_selected` to reflect the protocol's current
/// rule so `:gol rule` and the rule picker stay coherent.
pub(crate) fn apply_protocol_selection(
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

pub(crate) fn log_rule_overview(state: &mut AppState) {
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

pub(crate) fn log_rule_list(state: &mut AppState) {
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
