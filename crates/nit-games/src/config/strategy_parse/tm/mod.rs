use super::super::types::{StrategyConfig, StrategySpecKind};
use super::common::{parse_actions, parse_input_mode};
use crate::game::Action;
use crate::strategy::{symbol_to_action, InputMode, TmTransition};

mod explicit;
mod table;
mod wolfram;

/// Accepts either explicit transition rules (array-of-objects or table form)
/// or a Wolfram-style `rule_code`. The output map is forced to notebook
/// semantics (symbol 0 -> Cooperate, all others -> Defect).
pub(in crate::config) fn normalize_tm_kind(
    raw: &StrategyConfig,
    errors: &mut Vec<String>,
) -> StrategySpecKind {
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
    if let Some(mode) = parse_input_mode(id, raw.input_mode.as_deref(), errors) {
        if !matches!(mode, InputMode::OpponentLastAction) {
            errors.push(format!(
                "strategy '{id}': TM uses notebook semantics and ignores player perspective; use input_mode=opponent_last_action or omit it"
            ));
        }
    }

    validate_tm_range_bounds(
        id,
        states,
        symbols,
        start_state_raw,
        blank_raw,
        fallback_raw,
        errors,
    );
    let output_map = validate_tm_output_map(id, raw, symbols, errors);
    let transitions = resolve_tm_transitions(id, raw, states, symbols, blank_raw, errors);

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

fn validate_tm_range_bounds(
    id: &str,
    states: usize,
    symbols: usize,
    start_state: usize,
    blank: usize,
    fallback: usize,
    errors: &mut Vec<String>,
) {
    if states > 0 && (start_state == 0 || start_state > states) {
        errors.push(format!(
            "strategy '{id}': start_state must be in 1..={states}"
        ));
    }
    if symbols > 0 && blank >= symbols {
        errors.push(format!(
            "strategy '{id}': blank symbol {blank} out of range (symbols={symbols})"
        ));
    }
    if symbols > 0 && fallback >= symbols {
        errors.push(format!(
            "strategy '{id}': fallback_symbol {fallback} out of range (symbols={symbols})"
        ));
    }
}

/// Forces TM output map to notebook semantics: symbol 0 -> Cooperate, others -> Defect.
fn validate_tm_output_map(
    id: &str,
    raw: &StrategyConfig,
    symbols: usize,
    errors: &mut Vec<String>,
) -> Vec<Action> {
    let output_map_raw = raw
        .output_map
        .clone()
        .unwrap_or_else(|| vec!["C".to_string(), "D".to_string()]);
    let parsed = parse_actions(id, "output_map", output_map_raw, errors);

    if symbols > 0 && parsed.len() < symbols {
        errors.push(format!(
            "strategy '{id}': output_map length {} must be >= symbols {symbols}",
            parsed.len()
        ));
    }

    if symbols == 0 {
        return parsed;
    }

    let notebook: Vec<Action> = (0..symbols).map(|s| symbol_to_action(s as u8)).collect();

    if parsed.len() >= symbols && parsed[..symbols] != notebook[..symbols] {
        errors.push(format!(
            "strategy '{id}': output_map must map 0->C and all non-zero symbols->D to match notebook semantics"
        ));
    }
    notebook
}

fn resolve_tm_transitions(
    id: &str,
    raw: &StrategyConfig,
    states: usize,
    symbols: usize,
    blank: usize,
    errors: &mut Vec<String>,
) -> Vec<TmTransition> {
    let has_transitions = raw.transitions.is_some();
    let has_rule = raw.rule_code.is_some();
    if has_transitions && has_rule {
        errors.push(format!(
            "strategy '{id}': specify either transitions or rule_code, not both"
        ));
    }
    if let Some(value) = raw.transitions.clone() {
        parse_tm_transitions(id, value, states, symbols, blank, errors)
    } else if let Some(rule_code) = raw.rule_code {
        wolfram::decode_tm_rule_code(id, rule_code, states, symbols, errors)
    } else {
        errors.push(format!(
            "strategy '{id}': tm requires transitions or rule_code"
        ));
        Vec::new()
    }
}

fn parse_tm_transitions(
    id: &str,
    raw: toml::Value,
    states: usize,
    symbols: usize,
    blank: usize,
    errors: &mut Vec<String>,
) -> Vec<TmTransition> {
    // Try table form first (borrows); fall back to structured rules (consumes).
    if let Ok(transitions) = table::parse_tm_table_transitions(&raw, states, symbols) {
        return transitions;
    }
    explicit::apply_tm_transition_rules_from_value(id, raw, states, symbols, blank, errors)
}
