//! Strategy trait and shared types for game theory tournament strategies.

mod ca;
mod fsm;
pub(crate) mod math;
mod tm;

use serde::{Deserialize, Serialize};

use crate::game::Action;
use crate::history::History;

// ── Public re-exports (preserving lib.rs surface) ────────────

pub use ca::{decode_ca_rule_table, run_shrinking_ca, CaRunResult, CaStrategy};
pub use fsm::{decode_fsm_notebook_index, fsm_count, history_to_input_u64, FsmStrategy};
pub use tm::{
    run_one_sided_tm, run_one_sided_tm_from_integer, OneSidedTmStrategy, TmRunResult, TmRunStats,
    TmStopReason, TmTrace, TmTraceStep,
};

// ── Crate-internal re-exports ────────────────────────────────

pub(crate) use fsm::{decode_notebook_index_digits, validate_decode_params};
pub(crate) use tm::InputSuffix;

// ── Strategy trait ───────────────────────────────────────────

/// Core trait for iterated game strategies.
///
/// Each strategy maintains internal state and produces an [`Action`] per round
/// based on the accumulated [`History`] of play.
pub trait Strategy: Send {
    fn id(&self) -> &str;
    fn reset(&mut self);
    fn next_action(&mut self, history: &History, player_a: bool) -> Action;

    /// TM-specific: whether the strategy halted on its last evaluation.
    fn last_halted(&self) -> bool {
        true
    }

    /// TM-specific: accumulated runtime statistics.
    fn tm_stats(&self) -> Option<&TmRunStats> {
        None
    }
}

// ── Strategy kind ────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StrategyKind {
    Fsm,
    Ca,
    OneSidedTm,
}

// ── Input mode ───────────────────────────────────────────────

/// Which player perspective drives the input symbol fed to the strategy.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    OpponentLastAction,
    SelfLastAction,
    JointLastAction,
}

impl InputMode {
    pub fn alphabet_size(self) -> usize {
        match self {
            Self::OpponentLastAction | Self::SelfLastAction => 2,
            Self::JointLastAction => 4,
        }
    }
}

// ── TM types ─────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TmMove {
    #[serde(rename = "L")]
    Left,
    #[serde(rename = "R")]
    Right,
    #[serde(rename = "S")]
    Stay,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct TmTransition {
    pub write: u8,
    #[serde(rename = "move")]
    pub move_dir: TmMove,
    /// 1-indexed; 0 is the halt pseudo-state.
    pub next: u16,
}

// ── Codec functions ──────────────────────────────────────────

/// Decode a Wolfram-style rule code into a flat transition table.
/// `remaining` in the return is any unused higher-order digits.
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
    // Both dimensions are non-zero (early return above), so digit_radix cannot be 0.
    let digit_radix = (symbols as u64) * (states as u64) * 2;
    let mut undecoded_suffix = rule_code;
    for wolfram_state in (1..=states).rev() {
        for read_symbol in 0..symbols {
            let decoded_rule = decode_single_wolfram_digit(undecoded_suffix, digit_radix, symbols);
            undecoded_suffix /= digit_radix;
            let table_offset = (wolfram_state - 1) * symbols + read_symbol;
            transitions[table_offset] = decoded_rule;
        }
    }
    (transitions, undecoded_suffix)
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
    let mixed_radix = (2u128)
        .checked_mul(states as u128)?
        .checked_mul(symbols as u128)?;
    let total_entries = states.checked_mul(symbols)? as u32;
    math::checked_pow_u128(mixed_radix, total_entries)?.checked_sub(1)
}

// ── Internal helpers ─────────────────────────────────────────

pub(crate) fn symbol_to_action(symbol: u8) -> Action {
    match symbol {
        0 => Action::Cooperate,
        _ => Action::Defect,
    }
}

pub(crate) fn action_bit(action: Action) -> u8 {
    match action {
        Action::Cooperate => 0,
        Action::Defect => 1,
    }
}
