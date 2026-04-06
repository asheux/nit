//! Strategy trait and shared types for game theory tournament strategies.
//!
//! This module defines the [`Strategy`] trait, shared enumerations, and codec
//! functions. Concrete implementations live in submodules: [`fsm`], [`ca`],
//! and [`tm`].

mod ca;
mod fsm;
pub(crate) mod math;
mod tm;

use crate::game::Action;
use crate::history::History;
use serde::{Deserialize, Serialize};

// ── Public re-exports (preserving lib.rs surface) ────────────

pub use ca::{decode_ca_rule_table, run_shrinking_ca, CaRunResult, CaStrategy};
pub use fsm::{decode_fsm_notebook_index, fsm_count, history_to_input_u64, FsmStrategy};
pub use tm::{
    run_one_sided_tm, run_one_sided_tm_from_integer, OneSidedTmStrategy, TmRunResult, TmRunStats,
    TmStopReason, TmTrace, TmTraceStep,
};

// ── Crate-internal re-exports ────────────────────────────────

pub(crate) use tm::{tm_action_from_output_symbol, InputSuffix};

// ── Strategy trait ───────────────────────────────────────────

/// Core trait for iterated game strategies.
///
/// Each strategy maintains internal state and produces an [`Action`] per round
/// based on the accumulated [`History`] of play.
pub trait Strategy: Send {
    /// Unique identifier for this strategy instance.
    fn id(&self) -> &str;

    /// Reset internal state to initial conditions.
    fn reset(&mut self);

    /// Choose the next action given the game history and player role.
    fn next_action(&mut self, history: &History, player_a: bool) -> Action;

    /// Whether the strategy halted on its last evaluation (TM-specific).
    fn last_halted(&self) -> bool {
        true
    }

    /// Runtime statistics for TM-based strategies.
    fn tm_stats(&self) -> Option<&TmRunStats> {
        None
    }
}

// ── Strategy kind ────────────────────────────────────────────

/// Discriminant identifying the strategy family.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StrategyKind {
    /// Finite state machine strategy.
    Fsm,
    /// Cellular automaton strategy.
    Ca,
    /// One-sided Turing machine strategy.
    OneSidedTm,
}

// ── Input mode ───────────────────────────────────────────────

/// Determines which player perspective drives the input symbol.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    OpponentLastAction,
    SelfLastAction,
    JointLastAction,
}

impl InputMode {
    /// Number of distinct input symbols for this mode.
    pub fn alphabet_size(self) -> usize {
        match self {
            Self::OpponentLastAction | Self::SelfLastAction => 2,
            Self::JointLastAction => 4,
        }
    }
}

// ── TM types ─────────────────────────────────────────────────

/// Direction the TM head moves after a transition.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TmMove {
    /// Move the head one cell to the left.
    #[serde(rename = "L")]
    Left,
    /// Move the head one cell to the right.
    #[serde(rename = "R")]
    Right,
    /// Keep the head at the current cell.
    #[serde(rename = "S")]
    Stay,
}

/// A single TM transition rule: write symbol, move head, go to next state.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct TmTransition {
    /// Symbol to write on the tape at the current head position.
    pub write: u8,
    /// Direction to move the head after writing.
    #[serde(rename = "move")]
    pub move_dir: TmMove,
    /// 1-indexed state to transition to (0 is the halt pseudo-state).
    pub next: u16,
}

// ── Codec functions ──────────────────────────────────────────

/// Decode a Wolfram-style rule code into a flat transition table.
///
/// Returns `(transitions, remaining)` where `remaining` is any unused
/// higher-order digits from the rule code.
pub fn decode_tm_rule_code_wolfram(
    rule_code: u64,
    states: usize,
    symbols: usize,
) -> (Vec<TmTransition>, u64) {
    let entry_count = states.saturating_mul(symbols);
    let mut transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Left,
            next: 1,
        };
        entry_count
    ];
    if states == 0 || symbols == 0 {
        return (transitions, rule_code);
    }
    let mixed_radix_base = (symbols as u64) * (states as u64) * 2;
    if mixed_radix_base == 0 {
        return (transitions, rule_code);
    }
    let mut remaining_code = rule_code;
    for current_state in (1..=states).rev() {
        for tape_symbol in 0..symbols {
            let transition = decode_single_wolfram_digit(remaining_code, mixed_radix_base, symbols);
            remaining_code /= mixed_radix_base;

            let transition_index = (current_state - 1) * symbols + tape_symbol;
            if let Some(slot) = transitions.get_mut(transition_index) {
                *slot = transition;
            }
        }
    }
    (transitions, remaining_code)
}

/// Extract a single TM transition from the lowest digit of the rule code.
///
/// The digit is decomposed as `(move_flag, write_symbol, next_state)` in
/// mixed-radix form with base `2 * symbols * states`.
fn decode_single_wolfram_digit(
    remaining_code: u64,
    mixed_radix_base: u64,
    symbol_count: usize,
) -> TmTransition {
    let digit_value = remaining_code % mixed_radix_base;
    let move_flag = (digit_value % 2) as u8;
    let write_symbol = ((digit_value / 2) % symbol_count as u64) as u8;
    let next_state = (digit_value / (2 * symbol_count as u64)) as u16 + 1;
    let head_direction = if move_flag == 0 {
        TmMove::Left
    } else {
        TmMove::Right
    };
    TmTransition {
        write: write_symbol,
        move_dir: head_direction,
        next: next_state,
    }
}

/// Maximum valid Wolfram rule index for the given state/symbol counts.
pub fn tm_max_index(states: usize, symbols: usize) -> Option<u128> {
    let base = (2u128)
        .checked_mul(states as u128)?
        .checked_mul(symbols as u128)?;
    let exp = states.checked_mul(symbols)? as u32;
    math::checked_pow_u128(base, exp)?.checked_sub(1)
}

// ── Internal helpers ─────────────────────────────────────────

/// Map a game action to a single bit: Cooperate is 0, Defect is 1.
pub(crate) fn action_bit(action: Action) -> u8 {
    match action {
        Action::Cooperate => 0,
        Action::Defect => 1,
    }
}
