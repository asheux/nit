//! Finite state machine (FSM) strategy implementation.
//!
//! Provides the [`FsmStrategy`] type and codec functions for FSM notebook
//! index encoding, used by the tournament engine and introspection tooling.
//!
//! # Architecture
//!
//! An FSM strategy is defined by two components:
//!
//! - **Output function**: maps each internal state to a game [`Action`].
//! - **Transition function**: given the current state and an input symbol
//!   (derived from the opponent's previous action), determines the next state.
//!
//! These two functions are packed into a single integer index via mixed-radix
//! encoding (the "notebook index" scheme). The [`FsmDecodeResult`] struct
//! bundles the decoded output and transition data.

use super::math::{
    checked_pow_u128, floor_div_rem_i128, integer_digits_signed_abs, integer_digits_unsigned,
};
use crate::game::Action;
use crate::history::History;

// ── Decoded FSM representation ─────────────────────────────

/// Result of decoding an FSM notebook index.
///
/// Bundles the per-state output actions with the full state-by-input
/// transition table, ready for use by [`FsmStrategy`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FsmDecodeResult {
    /// Output action for each internal state.
    pub(crate) output_actions: Vec<Action>,
    /// Transition table indexed as `[state][input_symbol]`.
    pub(crate) transition_table: Vec<Vec<usize>>,
}

impl std::fmt::Display for FsmDecodeResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "FSM(outputs={}, states={})",
            self.output_actions.len(),
            self.transition_table.len()
        )
    }
}

// ── FSM enumeration ─────────────────────────────────────────

/// Count the total number of distinct FSM specifications for the given
/// `states` count and `actions` output alphabet size.
///
/// Each FSM is determined by a transition function (`states^(states*actions)`)
/// and an output function (`actions^states`). The product of these two
/// quantities gives the total FSM index space. Returns `None` on overflow.
///
/// # Examples
///
/// A 2-state, 2-action FSM has `2^(2*2) * 2^2 = 64` distinct specifications.
pub fn fsm_count(num_states: usize, alphabet_size: usize) -> Option<u128> {
    let transition_entries = num_states.checked_mul(alphabet_size)?;
    let total_transitions = checked_pow_u128(num_states as u128, transition_entries as u32)?;
    let output_assignments = checked_pow_u128(alphabet_size as u128, num_states as u32)?;
    total_transitions.checked_mul(output_assignments)
}

// ── Notebook codec ──────────────────────────────────────────

/// Decode a notebook-style FSM index into output actions and transition table.
///
/// The encoding packs both the output function and transition table into a
/// single integer, using mixed-radix decomposition.  Returns
/// `(outputs, transitions)` where each transition row maps input symbols to
/// next states.
///
/// # Errors
///
/// Returns an error string when `states` or `actions` is zero, or when the
/// `index` exceeds the FSM index space for the given dimensions.
pub fn decode_fsm_notebook_index(
    index: u64,
    states: usize,
    actions: usize,
) -> Result<(Vec<Action>, Vec<Vec<usize>>), String> {
    validate_decode_params(index, states, actions)?;

    let decoded = decode_notebook_index_inner(index, states, actions)?;
    Ok((decoded.output_actions, decoded.transition_table))
}

/// Core decoding logic, separated from the public API for testability.
///
/// Splits the packed integer into an output-function code and a
/// transition-function code, then decodes each independently.
fn decode_notebook_index_inner(
    index: u64,
    num_states: usize,
    action_count: usize,
) -> Result<FsmDecodeResult, String> {
    let flat_size = num_states.saturating_mul(action_count);
    let action_block_size = checked_pow_u128(action_count as u128, num_states as u32)
        .ok_or_else(|| "fsm action block overflows u128".to_string())?;
    let (transition_code, output_code) =
        floor_div_rem_i128(index as i128 - 1, action_block_size as i128);

    let output_actions = decode_output_actions(output_code as u128, num_states, action_count);
    let flat_next_states = decode_flat_transitions(transition_code, num_states, flat_size);
    let transition_table = reshape_transition_table(&flat_next_states, num_states, action_count);

    Ok(FsmDecodeResult {
        output_actions,
        transition_table,
    })
}

// ── Validation ─────────────────────────────────────────────

/// Validate the FSM decode parameters before any computation.
///
/// Checks that `states > 0`, `actions > 0`, and `index` falls within the
/// enumeration range for the given dimensions.
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
        Err(format!(
            "fsm index {index} out of range (0..{})",
            upper_bound - 1
        ))
    } else {
        Ok(())
    }
}

/// Convert output-code digits into cooperate/defect actions per state.
///
/// Each digit selects an action: 0 maps to [`Action::Cooperate`], any
/// non-zero digit maps to [`Action::Defect`].
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

/// Reshape flat next-state digits into a `states x actions` transition table.
///
/// Produces a 2D vector indexed as `[state][action]`. Any out-of-range
/// next-state values are clamped to the last valid state index.
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

/// Number of bits used to encode each round in [`history_to_input_u64`].
///
/// Each round contributes one bit for player A and one for player B,
/// giving a base-4 (2-bit) encoding per round.
const BITS_PER_ROUND: u32 = 2;

/// Base for the positional encoding of joint actions (2^BITS_PER_ROUND = 4).
const JOINT_ACTION_BASE: u64 = 1 << BITS_PER_ROUND;

/// Encode the joint action history as a single `u64` value.
///
/// Each round contributes two bits (player A high, player B low), accumulated
/// in base-4 encoding.  Returns `None` on arithmetic overflow.
///
/// # Overflow
///
/// With 2 bits per round, up to 32 rounds fit in a `u64`. Longer histories
/// cause this function to return `None`.
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
    ///
    /// Returns `None` when `input_symbol` falls outside the allocated table.
    fn lookup_transition(&self, input_symbol: usize) -> Option<usize> {
        let flat_idx = self
            .state
            .saturating_mul(self.alphabet)
            .saturating_add(input_symbol);
        self.transitions.get(flat_idx).copied()
    }

    /// Return the output action for the current internal state.
    ///
    /// Falls back to [`Action::Cooperate`] if the state index is out of range.
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
