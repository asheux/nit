use super::super::types::{StrategyConfig, StrategySpecKind};
use super::common::{parse_actions, parse_input_mode};
use crate::game::Action;
use crate::strategy::{
    decode_tm_rule_code_wolfram, symbol_to_action, InputMode, TmMove, TmTransition,
};
use serde::Deserialize;

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
        decode_tm_rule_code(id, rule_code, states, symbols, errors)
    } else {
        errors.push(format!(
            "strategy '{id}': tm requires transitions or rule_code"
        ));
        Vec::new()
    }
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
    // Try table format first (borrows), then structured rules (consumes).
    if let Ok(transitions) = parse_tm_table_transitions(&raw, states, symbols) {
        return transitions;
    }

    match raw.try_into::<Vec<TmTransitionRule>>() {
        Ok(rules) => apply_tm_transition_rules(id, &rules, states, symbols, blank, errors),
        Err(err) => {
            errors.push(format!("strategy '{id}': invalid tm transitions: {err}"));
            Vec::new()
        }
    }
}

fn validate_tm_rule_bounds(
    id: &str,
    rule: &TmTransitionRule,
    states: usize,
    symbols: usize,
    errors: &mut Vec<String>,
) -> Option<usize> {
    let TmTransitionRule {
        state,
        read,
        write,
        next,
        ..
    } = *rule;
    if state == 0 || state > states {
        errors.push(format!(
            "strategy '{id}': tm transition state {state} out of range (1..={states})"
        ));
        return None;
    }
    if read >= symbols {
        errors.push(format!(
            "strategy '{id}': tm transition read {read} out of range (symbols={symbols})"
        ));
        return None;
    }
    if write >= symbols {
        errors.push(format!(
            "strategy '{id}': tm transition write {write} out of range (symbols={symbols})"
        ));
        return None;
    }
    if next > states {
        errors.push(format!(
            "strategy '{id}': tm transition next {next} out of range (0..={states})"
        ));
        return None;
    }
    Some((state - 1) * symbols + read)
}

fn apply_tm_transition_rules(
    id: &str,
    rules: &[TmTransitionRule],
    states: usize,
    symbols: usize,
    blank: usize,
    errors: &mut Vec<String>,
) -> Vec<TmTransition> {
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
        let Some(idx) = validate_tm_rule_bounds(id, rule, states, symbols, errors) else {
            continue;
        };
        match seen.get_mut(idx) {
            Some(slot) if *slot => {
                errors.push(format!(
                    "strategy '{id}': duplicate tm transition for state {} read {}",
                    rule.state, rule.read
                ));
                continue;
            }
            Some(slot) => *slot = true,
            None => continue,
        }
        if let Some(entry) = transitions.get_mut(idx) {
            *entry = TmTransition {
                write: rule.write as u8,
                move_dir: rule.move_dir,
                next: rule.next as u16,
            };
        }
    }
    let missing = seen.iter().filter(|&&v| !v).count();
    if missing > 0 {
        errors.push(format!(
            "strategy '{id}': tm transitions missing {missing} (state, read) pairs"
        ));
    }
    transitions
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
            let idx = state_idx * symbols + read_idx;
            transitions[idx] =
                parse_tm_table_cell(entry_val, state_idx, read_idx, states, symbols)?;
        }
    }
    Ok(transitions)
}

fn parse_tm_table_cell(
    entry_val: &toml::Value,
    state_idx: usize,
    read_idx: usize,
    states: usize,
    symbols: usize,
) -> Result<TmTransition, String> {
    let entry = entry_val.as_array().ok_or_else(|| {
        format!("transitions[{state_idx}][{read_idx}] must be [next, write, move]")
    })?;
    if entry.len() != 3 {
        return Err(format!(
            "transitions[{state_idx}][{read_idx}] must be [next, write, move]"
        ));
    }
    let next = entry[0]
        .as_integer()
        .ok_or_else(|| format!("transitions[{state_idx}][{read_idx}][0] must be an integer"))?;
    let write = entry[1]
        .as_integer()
        .ok_or_else(|| format!("transitions[{state_idx}][{read_idx}][1] must be an integer"))?;
    let move_dir = parse_tm_move_value(&entry[2], state_idx, read_idx)?;
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
    Ok(TmTransition {
        write: write as u8,
        move_dir,
        next: next as u16,
    })
}

fn parse_tm_move_value(
    value: &toml::Value,
    state_idx: usize,
    read_idx: usize,
) -> Result<TmMove, String> {
    if let Some(move_int) = value.as_integer() {
        return match move_int {
            -1 => Ok(TmMove::Left),
            0 => Ok(TmMove::Stay),
            1 => Ok(TmMove::Right),
            other => Err(format!(
                "transitions[{state_idx}][{read_idx}][2] invalid move {other} (expected -1, 0, or 1)"
            )),
        };
    }
    if let Some(move_str) = value.as_str() {
        let normalized = move_str.trim().to_ascii_lowercase();
        return match normalized.as_str() {
            "l" | "left" => Ok(TmMove::Left),
            "r" | "right" => Ok(TmMove::Right),
            "s" | "stay" => Ok(TmMove::Stay),
            _ => Err(format!(
                "transitions[{state_idx}][{read_idx}][2] invalid move '{normalized}'"
            )),
        };
    }
    Err(format!(
        "transitions[{state_idx}][{read_idx}][2] must be a move string or integer"
    ))
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
