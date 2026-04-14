//! Strategy specification parsing: FSM, CA, and TM normalization from TOML.

use super::types::{StrategyConfig, StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::math::checked_pow_u128;
use crate::strategy::{
    decode_fsm_notebook_index, decode_tm_rule_code_wolfram, symbol_to_action, InputMode, TmMove,
    TmTransition,
};
use serde::Deserialize;
use std::io::BufRead;
use std::path::Path;

pub(super) fn normalize_fsm_kind(
    raw: &StrategyConfig,
    errors: &mut Vec<String>,
) -> StrategySpecKind {
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
        normalize_fsm_from_index(raw, index, errors)
    } else {
        normalize_fsm_from_explicit(raw, errors)
    }
}

fn normalize_fsm_from_index(
    raw: &StrategyConfig,
    index: u64,
    errors: &mut Vec<String>,
) -> StrategySpecKind {
    let id = raw.id.as_str();
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
}

fn normalize_fsm_from_explicit(raw: &StrategyConfig, errors: &mut Vec<String>) -> StrategySpecKind {
    let id = raw.id.as_str();
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
    let start_state = normalize_index(id, "start_state", start_state_raw, input_index_base, errors);
    if start_state >= num_states && num_states > 0 {
        errors.push(format!(
            "strategy '{id}': start_state {start_state} out of range"
        ));
    }
    validate_fsm_transitions(id, &transitions, num_states, errors);
    StrategySpecKind::Fsm {
        num_states,
        start_state,
        outputs,
        input_mode: Some(InputMode::OpponentLastAction),
        transitions,
        index: None,
    }
}

fn validate_fsm_transitions(
    id: &str,
    transitions: &[Vec<usize>],
    num_states: usize,
    errors: &mut Vec<String>,
) {
    if num_states == 0 {
        return;
    }
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

pub(super) fn normalize_ca_kind(
    raw: &StrategyConfig,
    errors: &mut Vec<String>,
) -> StrategySpecKind {
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
    let two_r = match neighborhood_radius_to_diameter(r_raw) {
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

/// Accepts either explicit transition rules (array-of-objects or table form)
/// or a Wolfram-style `rule_code`. The output map is forced to notebook
/// semantics (symbol 0 -> Cooperate, all others -> Defect).
pub(super) fn normalize_tm_kind(
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
    let parsed_mode = parse_input_mode(id, raw.input_mode.as_deref(), errors);
    if let Some(mode) = parsed_mode {
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

/// Validates and normalizes the TM output map to notebook semantics:
/// symbol 0 -> Cooperate, all others -> Defect.
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

    // Build the canonical notebook mapping and validate against user input.
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

fn parse_actions(
    id: &str,
    field: &str,
    values: Vec<String>,
    errors: &mut Vec<String>,
) -> Vec<Action> {
    values
        .into_iter()
        .filter_map(|value| {
            Action::parse(&value).or_else(|| {
                errors.push(format!(
                    "strategy '{id}': invalid action '{value}' in {field}"
                ));
                None
            })
        })
        .collect()
}

fn normalize_index(
    id: &str,
    field: &str,
    value: usize,
    input_index_base: u8,
    errors: &mut Vec<String>,
) -> usize {
    if input_index_base != 1 {
        return value;
    }
    if value == 0 {
        errors.push(format!(
            "strategy '{id}': {field} must be >= 1 when input_index_base = 1"
        ));
        return 0;
    }
    value - 1
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

pub(super) fn load_generated_strategies(
    id: &str,
    source: Option<&str>,
    limit: Option<usize>,
    base_dir: Option<&Path>,
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

/// Converts CA neighbourhood radius to diameter (`2r`) as an integer.
/// Returns `None` when the input is invalid (non-finite, negative, or
/// non-integer when doubled).
fn neighborhood_radius_to_diameter(r: f32) -> Option<u32> {
    if !r.is_finite() || r < 0.0 {
        return None;
    }
    let doubled = r * 2.0;
    let rounded = doubled.round();
    if (doubled - rounded).abs() > 1e-6 || rounded < 0.0 {
        return None;
    }
    Some(rounded as u32)
}
