//! One-sided Turing machine strategy: types, evaluation engine, and a
//! `Strategy` adapter that drives the engine over the joint action history.

pub(crate) mod engine;
pub(crate) mod input_suffix;
mod strategy_impl;

use super::TmMove;

pub use engine::{run_one_sided_tm, run_one_sided_tm_from_integer};
pub use strategy_impl::OneSidedTmStrategy;

pub(crate) use input_suffix::InputSuffix;

#[derive(Clone, Debug, Default)]
pub struct TmRunStats {
    pub rounds: u64,
    pub steps: u64,
    pub min_steps: u32,
    pub max_steps: u32,
    pub output_events: u64,
    pub fallback: u64,
    pub max_steps_hits: u64,
}

impl TmRunStats {
    pub(crate) fn update_step_range(&mut self, min: u32, max: u32) {
        if self.rounds == 0 {
            self.min_steps = min;
            self.max_steps = max;
        } else {
            self.min_steps = self.min_steps.min(min);
            self.max_steps = self.max_steps.max(max);
        }
    }

    pub fn merge(&mut self, incoming: &Self) {
        if incoming.rounds > 0 {
            self.update_step_range(incoming.min_steps, incoming.max_steps);
        }
        self.rounds = self.rounds.saturating_add(incoming.rounds);
        self.steps = self.steps.saturating_add(incoming.steps);
        self.output_events = self.output_events.saturating_add(incoming.output_events);
        self.fallback = self.fallback.saturating_add(incoming.fallback);
        self.max_steps_hits = self.max_steps_hits.saturating_add(incoming.max_steps_hits);
    }
}

#[derive(Clone, Debug)]
pub struct TmTraceStep {
    pub step: usize,
    pub state: u16,
    pub head_before: usize,
    pub read: u8,
    pub next: u16,
    pub write: u8,
    pub move_dir: TmMove,
    pub head_after: usize,
    pub tape: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct TmTrace {
    pub input_digits: Vec<u8>,
    pub initial_tape: Vec<u8>,
    pub initial_head: usize,
    pub steps: Vec<TmTraceStep>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TmStopReason {
    Output,
    MaxSteps,
    MissingTransition,
    InvalidState,
}

#[derive(Clone, Debug)]
pub struct TmRunResult {
    pub output_value: Option<u64>,
    pub output_symbol: Option<u8>,
    pub halted: bool,
    pub steps_taken: u32,
    pub stop_reason: TmStopReason,
    pub trace: Option<TmTrace>,
}

impl TmRunResult {
    pub(crate) fn terminated(
        steps_taken: u32,
        stop_reason: TmStopReason,
        trace: Option<TmTrace>,
    ) -> Self {
        Self {
            output_value: None,
            output_symbol: None,
            halted: false,
            steps_taken,
            stop_reason,
            trace,
        }
    }
}
