//! Enumeration, canonicalisation, and behavioural grouping of FSM strategies.

mod behavior_trace;
mod cache;
mod canonical;
mod minimize;

use crate::config::{StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::InputMode;
use nit_utils::hashing::stable_hash_bytes;

pub use behavior_trace::{
    group_canonical_fsm_indices_by_behavior, group_canonical_fsm_indices_by_behavior_with_mode,
    unique_fsm_behavior_representatives, unique_fsm_behavior_representatives_with_mode,
};
pub use canonical::{canonical_fsm_indices, canonicalize_fsm, enumerate_fsms};

#[derive(Clone, Debug)]
pub struct FsmDefinition {
    pub num_states: usize,
    pub start_state: usize,
    pub outputs: Vec<Action>,
    pub input_mode: InputMode,
    pub transitions: Vec<Vec<usize>>,
}

impl FsmDefinition {
    pub fn to_spec(&self, id: String) -> StrategySpec {
        StrategySpec {
            id,
            name: None,
            kind: StrategySpecKind::Fsm {
                num_states: self.num_states,
                start_state: self.start_state,
                outputs: self.outputs.clone(),
                input_mode: Some(self.input_mode),
                transitions: self.transitions.clone(),
                index: None,
            },
        }
    }

    /// Hash-stable string key for caching and equivalence checks. Format:
    /// `mode=<id>;states=<n>;start=<s>;outputs=<chars>;transitions=<row|row>`.
    pub fn stable_key(&self) -> String {
        let mode_tag = match self.input_mode {
            InputMode::OpponentLastAction => "opp",
            InputMode::SelfLastAction => "self",
            InputMode::JointLastAction => "joint",
        };
        let outputs: String = self.outputs.iter().map(|a| a.as_char()).collect();
        let transitions = self
            .transitions
            .iter()
            .map(|row| {
                row.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("|");
        format!(
            "mode={mode_tag};states={};start={};outputs={outputs};transitions={transitions}",
            self.num_states, self.start_state,
        )
    }

    pub fn stable_hash(&self) -> u64 {
        stable_hash_bytes(self.stable_key().as_bytes())
    }
}

/// Internal FSM with action-index outputs (rather than `Action`) so
/// canonicalisation, minimisation, and trace signatures stay in plain
/// `usize` arithmetic.
#[derive(Clone, Debug)]
struct RawFsm {
    outputs: Vec<usize>,
    transitions: Vec<Vec<usize>>,
    actions: usize,
}

impl RawFsm {
    fn states(&self) -> usize {
        self.outputs.len()
    }
}
