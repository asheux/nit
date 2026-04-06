//! Finite state machine (FSM) strategy implementation.
//!
//! Provides the [`FsmStrategy`] type and codec functions for FSM notebook
//! index encoding, used by the tournament engine and introspection tooling.

use super::math::{
    checked_pow_u128, floor_div_rem_i128, integer_digits_signed_abs, integer_digits_unsigned,
};
use crate::game::Action;
use crate::history::History;

// ── FSM enumeration ─────────────────────────────────────────

/// Count the total number of distinct FSM specifications for the given
/// `states` count and `actions` output alphabet size.
///
/// Each FSM is determined by a transition function (`states^(states*actions)`)
/// and an output function (`actions^states`).  Returns `None` on overflow.
pub fn fsm_count(states: usize, actions: usize) -> Option<u128> {
    let transition_entries = states.checked_mul(actions)?;
    let total_transitions = checked_pow_u128(states as u128, transition_entries as u32)?;
    let output_assignments = checked_pow_u128(actions as u128, states as u32)?;
    total_transitions.checked_mul(output_assignments)
}

// ── Notebook codec ──────────────────────────────────────────

/// Decode a notebook-style FSM index into output actions and transition table.
///
/// The encoding packs both the output function and transition table into a
/// single integer, using mixed-radix decomposition.  Returns
/// `(outputs, transitions)` where each transition row maps input symbols to
/// next states.
pub fn decode_fsm_notebook_index(
    index: u64,
    states: usize,
    actions: usize,
) -> Result<(Vec<Action>, Vec<Vec<usize>>), String> {
    validate_decode_params(index, states, actions)?;

    let flat_size = states.saturating_mul(actions);
    let action_block = checked_pow_u128(actions as u128, states as u32)
        .ok_or_else(|| "fsm action block overflows u128".to_string())?;
    let (transition_code, output_code) =
        floor_div_rem_i128(index as i128 - 1, action_block as i128);

    let output_actions = decode_output_actions(output_code as u128, states, actions);
    let flat_next = decode_flat_transitions(transition_code, states, flat_size);
    let transition_table = reshape_transition_table(&flat_next, states, actions);

    Ok((output_actions, transition_table))
}

/// Validate the FSM decode parameters before any computation.
fn validate_decode_params(index: u64, states: usize, actions: usize) -> Result<(), String> {
    if states == 0 {
        return Err("fsm decode requires states > 0".to_string());
    }
    if actions == 0 {
        return Err("fsm decode requires actions > 0".to_string());
    }
    let upper_bound = fsm_count(states, actions)
        .ok_or_else(|| "fsm index space overflows u128 for this (states, actions)".to_string())?;
    if (index as u128) >= upper_bound {
        Err(format!("fsm index {index} out of range (0..{})", upper_bound - 1))
    } else {
        Ok(())
    }
}

/// Convert output-code digits into cooperate/defect actions per state.
fn decode_output_actions(output_code: u128, num_states: usize, num_actions: usize) -> Vec<Action> {
    let raw_digits = if num_actions <= 1 {
        vec![0usize; num_states]
    } else {
        integer_digits_unsigned(output_code, num_actions, num_states)
    };
    raw_digits
        .into_iter()
        .map(|digit| match digit {
            0 => Action::Cooperate,
            _ => Action::Defect,
        })
        .collect()
}

/// Decode the transition function as a flat array of next-state indices.
///
/// For single-state machines, every entry maps to state 0.
fn decode_flat_transitions(
    transition_code: i128,
    num_states: usize,
    flat_size: usize,
) -> Vec<usize> {
    match num_states {
        0 | 1 => vec![0usize; flat_size],
        _ => integer_digits_signed_abs(transition_code, num_states, flat_size),
    }
}

/// Reshape flat next-state digits into a `states × actions` transition table,
/// clamping any out-of-range values to the last valid state index.
fn reshape_transition_table(
    flat_next: &[usize],
    num_states: usize,
    num_actions: usize,
) -> Vec<Vec<usize>> {
    let max_valid_state = num_states.saturating_sub(1);
    (0..num_states)
        .map(|state_idx| {
            (0..num_actions)
                .map(|action_idx| {
                    let offset = state_idx
                        .saturating_mul(num_actions)
                        .saturating_add(action_idx);
                    flat_next.get(offset).copied().unwrap_or(0).min(max_valid_state)
                })
                .collect()
        })
        .collect()
}

// ── History encoding ────────────────────────────────────────

/// Encode the joint action history as a single `u64` value.
///
/// Each round contributes two bits (player A high, player B low), accumulated
/// in base-4 encoding.  Returns `None` on arithmetic overflow.
pub fn history_to_input_u64(history: &History) -> Option<u64> {
    let mut encoded_value: u64 = 0;
    for round in history.iter() {
        let high_bit = super::action_bit(round.a) as u64;
        let low_bit = super::action_bit(round.b) as u64;
        let pair = (high_bit << 1) | low_bit;
        encoded_value = encoded_value.checked_mul(4)?.checked_add(pair)?;
    }
    Some(encoded_value)
}

// ── FSM strategy ────────────────────────────────────────────

/// FSM-based strategy: maps opponent history through state transitions to actions.
///
/// On each round, the opponent's previous action selects a transition from
/// the current state, updating the internal state and producing an output.
#[derive(Clone, Debug)]
pub struct FsmStrategy {
    id: String,
    start_state: usize,
    state: usize,
    outputs: Vec<Action>,
    transitions: Vec<usize>,
    alphabet: usize,
}

impl FsmStrategy {
    /// Construct from output actions and a 2D transition table.
    ///
    /// The transition rows are flattened into row-major order for
    /// cache-friendly lookup during gameplay.
    pub fn new(
        id: impl Into<String>,
        start_state: usize,
        outputs: Vec<Action>,
        input_mode: super::InputMode,
        transitions: Vec<Vec<usize>>,
    ) -> Self {
        let alphabet = match transitions.first() {
            Some(first_row) => first_row.len().max(1),
            None => input_mode.alphabet_size().max(2),
        };
        let flat_table: Vec<usize> = transitions.into_iter().flatten().collect();
        Self {
            id: id.into(),
            start_state,
            state: start_state,
            outputs,
            transitions: flat_table,
            alphabet,
        }
    }

    /// Look up the next state for the given input symbol in the flat table.
    fn lookup_transition(&self, input_symbol: usize) -> Option<usize> {
        let flat_idx = self
            .state
            .saturating_mul(self.alphabet)
            .saturating_add(input_symbol);
        self.transitions.get(flat_idx).copied()
    }

    /// Return the output action for the current internal state.
    fn current_output(&self) -> Action {
        self.outputs
            .get(self.state)
            .copied()
            .unwrap_or(Action::Cooperate)
    }
}

impl super::Strategy for FsmStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.state = self.start_state;
    }

    fn next_action(&mut self, history: &History, player_a: bool) -> Action {
        if let Some(last_round) = history.last() {
            let opponent_action = if player_a { last_round.b } else { last_round.a };
            let input_symbol = super::action_bit(opponent_action) as usize;
            if let Some(next) = self.lookup_transition(input_symbol) {
                self.state = next;
            }
        }
        self.current_output()
    }
}
