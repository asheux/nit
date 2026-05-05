//! Strategy trait and shared types for game theory tournament strategies.

mod ca;
mod fsm;
pub(crate) mod math;
mod tm;
mod wolfram_codec;

use serde::{Deserialize, Serialize};

use crate::game::Action;
use crate::history::History;

pub use ca::{decode_ca_rule_table, run_shrinking_ca, CaRunResult, CaStrategy};
pub use fsm::{decode_fsm_notebook_index, fsm_count, history_to_input_u64, FsmStrategy};
pub use tm::{
    run_one_sided_tm, run_one_sided_tm_from_integer, OneSidedTmStrategy, TmRunResult, TmRunStats,
    TmStopReason, TmTrace, TmTraceStep,
};
pub use wolfram_codec::{decode_tm_rule_code_wolfram, tm_max_index};

pub(crate) use fsm::{decode_notebook_index_digits, validate_decode_params};
pub(crate) use tm::InputSuffix;

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StrategyKind {
    Fsm,
    Ca,
    OneSidedTm,
}

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
