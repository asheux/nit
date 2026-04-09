//! Finite state machine (FSM) strategy implementation.
//!
//! An FSM strategy pairs an output function (state → action) with a transition
//! function (state × input → state). Both are packed into a single integer via
//! mixed-radix "notebook index" encoding.

use super::math::{
    checked_pow_u128, floor_div_rem_i128, integer_digits_signed_abs, integer_digits_unsigned,
};
use crate::game::Action;
use crate::history::History;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FsmDecodeResult {
    pub(crate) output_actions: Vec<Action>,
    pub(crate) transition_table: Vec<Vec<usize>>,
}

/// Total distinct FSMs: `states^(states*alphabet) * alphabet^states`.
/// Returns `None` on overflow.
pub fn fsm_count(num_states: usize, alphabet_size: usize) -> Option<u128> {
    let transition_entries = num_states.checked_mul(alphabet_size)?;
    let total_transitions = checked_pow_u128(num_states as u128, transition_entries as u32)?;
    let output_assignments = checked_pow_u128(alphabet_size as u128, num_states as u32)?;
    total_transitions.checked_mul(output_assignments)
}

pub fn decode_fsm_notebook_index(
    index: u64,
    states: usize,
    actions: usize,
) -> Result<(Vec<Action>, Vec<Vec<usize>>), String> {
    validate_decode_params(index, states, actions)?;

    let decoded = decode_notebook_index_inner(index, states, actions)?;
    Ok((decoded.output_actions, decoded.transition_table))
}

/// Decode notebook index into raw numeric output digits and transition table.
/// Preserves original digit values without converting to Action — used by
/// fsm_enum for canonicalization where raw digit identity matters for k>2.
pub(crate) fn decode_notebook_index_digits(
    index: u64,
    num_states: usize,
    action_count: usize,
) -> Result<(Vec<usize>, Vec<Vec<usize>>), String> {
    let flat_size = num_states.saturating_mul(action_count);
    let action_block_size = checked_pow_u128(action_count as u128, num_states as u32)
        .ok_or_else(|| "fsm action block overflows u128".to_string())?;
    let (transition_code, output_code) =
        floor_div_rem_i128(index as i128 - 1, action_block_size as i128);

    let output_digits = if action_count <= 1 {
        vec![0usize; num_states]
    } else {
        integer_digits_unsigned(output_code as u128, action_count, num_states)
    };
    let flat_next_states = decode_flat_transitions(transition_code, num_states, flat_size);
    let transition_table = reshape_transition_table(&flat_next_states, num_states, action_count);

    Ok((output_digits, transition_table))
}

fn decode_notebook_index_inner(
    index: u64,
    num_states: usize,
    action_count: usize,
) -> Result<FsmDecodeResult, String> {
    let (output_digits, transition_table) =
        decode_notebook_index_digits(index, num_states, action_count)?;
    let output_actions = output_digits
        .into_iter()
        .map(|digit| super::symbol_to_action(digit as u8))
        .collect();
    Ok(FsmDecodeResult {
        output_actions,
        transition_table,
    })
}

pub(crate) fn validate_decode_params(
    index: u64,
    states: usize,
    actions: usize,
) -> Result<(), String> {
    if states == 0 {
        return Err("fsm decode requires states > 0".to_string());
    }
    if actions == 0 {
        return Err("fsm decode requires actions > 0".to_string());
    }
    let upper_bound = fsm_count(states, actions)
        .ok_or_else(|| "fsm index space overflows u128 for this (states, actions)".to_string())?;
    if (index as u128) >= upper_bound {
        Err(format!(
            "fsm index {index} out of range (0..{})",
            upper_bound - 1
        ))
    } else {
        Ok(())
    }
}

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

/// Reshape flat next-state digits into a `[state][action]` table, clamping
/// out-of-range values to the last valid state.
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
                    flat_next
                        .get(offset)
                        .copied()
                        .unwrap_or(0)
                        .min(max_valid_state)
                })
                .collect()
        })
        .collect()
}

// ── History encoding ────────────────────────────────────────

const BITS_PER_ROUND: u32 = 2;
const JOINT_ACTION_BASE: u64 = 1 << BITS_PER_ROUND;

/// Encode the joint action history as a single `u64` (base-4, 2 bits/round).
/// Returns `None` on overflow (>32 rounds).
pub fn history_to_input_u64(history: &History) -> Option<u64> {
    let mut accumulated: u64 = 0;

    for round in history.iter() {
        let player_a_bit = super::action_bit(round.a) as u64;
        let player_b_bit = super::action_bit(round.b) as u64;
        let joint_pair = (player_a_bit << 1) | player_b_bit;

        accumulated = accumulated
            .checked_mul(JOINT_ACTION_BASE)?
            .checked_add(joint_pair)?;
    }

    Some(accumulated)
}

// ── FSM strategy ────────────────────────────────────────────

/// Maps opponent actions through state transitions to produce game actions.
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
    /// Flattens the 2D transition table into row-major order for cache-friendly lookup.
    pub fn new(
        id: impl Into<String>,
        start_state: usize,
        outputs: Vec<Action>,
        input_mode: super::InputMode,
        transitions: Vec<Vec<usize>>,
    ) -> Self {
        // Derive alphabet from actual transitions; fall back to input_mode
        // when transitions are empty (e.g. single-state strategies).
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

    fn lookup_transition(&self, input_symbol: usize) -> Option<usize> {
        let flat_idx = self
            .state
            .saturating_mul(self.alphabet)
            .saturating_add(input_symbol);
        self.transitions.get(flat_idx).copied()
    }

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
        let Some(last_round) = history.last() else {
            return self.current_output();
        };

        let opponent_action = if player_a { last_round.b } else { last_round.a };
        let input_symbol = super::action_bit(opponent_action) as usize;

        if let Some(next_state) = self.lookup_transition(input_symbol) {
            self.state = next_state;
        }

        self.current_output()
    }
}
