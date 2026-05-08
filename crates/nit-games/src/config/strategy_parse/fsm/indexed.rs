use super::super::super::types::{StrategyConfig, StrategySpecKind};
use crate::strategy::{decode_fsm_notebook_index, InputMode};

pub(super) fn normalize_fsm_from_index(
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
        return empty_indexed_spec(index);
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

fn empty_indexed_spec(index: u64) -> StrategySpecKind {
    StrategySpecKind::Fsm {
        num_states: 0,
        start_state: 0,
        outputs: Vec::new(),
        input_mode: Some(InputMode::OpponentLastAction),
        transitions: Vec::new(),
        index: Some(index),
    }
}
