use super::super::super::types::{StrategyConfig, StrategySpecKind};
use super::super::common::parse_actions;
use super::normalize_index;
use crate::strategy::InputMode;

pub(super) fn normalize_fsm_from_explicit(
    raw: &StrategyConfig,
    errors: &mut Vec<String>,
) -> StrategySpecKind {
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
