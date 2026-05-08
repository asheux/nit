use super::super::types::{StrategyConfig, StrategySpecKind};
use super::common::parse_input_mode;
use crate::strategy::InputMode;

mod explicit;
mod indexed;

pub(in crate::config) fn normalize_fsm_kind(
    raw: &StrategyConfig,
    errors: &mut Vec<String>,
) -> StrategySpecKind {
    let id = raw.id.as_str();
    if let Some(mode) = parse_input_mode(id, raw.input_mode.as_deref(), errors) {
        if !matches!(mode, InputMode::OpponentLastAction) {
            errors.push(format!(
                "strategy '{id}': FSM uses notebook semantics and only supports input_mode=opponent_last_action"
            ));
        }
    }

    match raw.index {
        Some(index) => indexed::normalize_fsm_from_index(raw, index, errors),
        None => explicit::normalize_fsm_from_explicit(raw, errors),
    }
}

/// Translates 1-based input indices back to the 0-based internal representation.
/// Returns 0 and pushes an error when `value == 0` while `input_index_base == 1`.
pub(super) fn normalize_index(
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
