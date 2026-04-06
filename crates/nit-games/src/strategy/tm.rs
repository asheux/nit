//! One-sided Turing machine (TM) strategy implementation.
//!
//! Provides [`OneSidedTmStrategy`], the TM evaluation engine, trace capture,
//! and the [`InputSuffix`] type used for incremental history encoding.

use super::math::{digits_in_base, digits_to_u64};
use crate::game::Action;
use crate::history::{History, RoundRecord};

// ── TM run statistics ────────────────────────────────────────

/// Accumulated runtime statistics for a TM strategy across multiple rounds.
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
    /// Merge another set of stats into this one (additive, min/max preserved).
    pub fn merge(&mut self, other: &TmRunStats) {
        if other.rounds > 0 {
            if self.rounds == 0 {
                self.min_steps = other.min_steps;
                self.max_steps = other.max_steps;
            } else {
                self.min_steps = self.min_steps.min(other.min_steps);
                self.max_steps = self.max_steps.max(other.max_steps);
            }
        }
        self.rounds = self.rounds.saturating_add(other.rounds);
        self.steps = self.steps.saturating_add(other.steps);
        self.output_events = self.output_events.saturating_add(other.output_events);
        self.fallback = self.fallback.saturating_add(other.fallback);
        self.max_steps_hits = self.max_steps_hits.saturating_add(other.max_steps_hits);
    }
}

// ── TM trace types ───────────────────────────────────────────

/// A single step in a TM execution trace.
#[derive(Clone, Debug)]
pub struct TmTraceStep {
    pub step: usize,
    pub state: u16,
    pub head_before: usize,
    pub read: u8,
    pub next: u16,
    pub write: u8,
    pub move_dir: super::TmMove,
    pub head_after: usize,
    pub tape: Vec<u8>,
}

/// Full execution trace of a TM run, including initial tape and all steps.
#[derive(Clone, Debug)]
pub struct TmTrace {
    pub input_digits: Vec<u8>,
    pub initial_tape: Vec<u8>,
    pub initial_head: usize,
    pub steps: Vec<TmTraceStep>,
}

/// Reason a TM run terminated.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TmStopReason {
    Output,
    MaxSteps,
    MissingTransition,
    InvalidState,
}

/// Result of running a one-sided TM on an input.
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
    /// Construct a non-output termination result (max-steps, invalid-state, etc.).
    ///
    /// Most TM exits produce no output value — only the "Output" stop reason does.
    /// This constructor captures the common pattern for all other exit paths.
    fn terminated(steps_taken: u32, stop_reason: TmStopReason, trace: Option<TmTrace>) -> Self {
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

// ── TM evaluation engine ─────────────────────────────────────

/// Run a one-sided TM with integer input converted to digit form.
pub fn run_one_sided_tm_from_integer(
    transitions: &[super::TmTransition],
    symbols: u8,
    start_state: u16,
    blank: u8,
    input: u64,
    max_steps: u32,
    with_trace: bool,
) -> TmRunResult {
    let digits = digits_in_base(input, symbols.max(2));
    run_one_sided_tm(
        transitions,
        symbols,
        start_state,
        blank,
        &digits,
        max_steps,
        with_trace,
    )
}

/// Run a one-sided Turing machine on explicit input digits.
///
/// The TM halts (with output) when the head moves right past the last tape cell.
/// Returns [`TmRunResult`] with execution outcome and optional trace.
pub fn run_one_sided_tm(
    transitions: &[super::TmTransition],
    symbols: u8,
    start_state: u16,
    blank: u8,
    input_digits: &[u8],
    max_steps: u32,
    with_trace: bool,
) -> TmRunResult {
    let symbols = symbols.max(1);
    let digits = if input_digits.is_empty() {
        vec![0]
    } else {
        input_digits.to_vec()
    };
    let mut tape = digits.clone();
    let mut head = tape.len().saturating_sub(1);
    let mut state = start_state;

    let mut trace = if with_trace {
        Some(TmTrace {
            input_digits: digits.clone(),
            initial_tape: tape.clone(),
            initial_head: head,
            steps: Vec::with_capacity(max_steps.min(10_000) as usize),
        })
    } else {
        None
    };

    if max_steps == 0 {
        return TmRunResult::terminated(0, TmStopReason::MaxSteps, trace);
    }
    if state == 0 {
        return TmRunResult::terminated(0, TmStopReason::InvalidState, trace);
    }

    for step in 0..(max_steps as usize) {
        let head_before = head;
        let read = tape.get(head_before).copied().unwrap_or(blank);
        let idx = (state.saturating_sub(1) as usize)
            .saturating_mul(symbols as usize)
            .saturating_add(read as usize);
        let Some(trans) = transitions.get(idx).copied() else {
            return TmRunResult::terminated(
                (step + 1) as u32,
                TmStopReason::MissingTransition,
                trace,
            );
        };

        if let Some(cell) = tape.get_mut(head_before) {
            *cell = trans.write;
        }

        if matches!(trans.move_dir, super::TmMove::Right) && head_before + 1 == tape.len() {
            let output_value = digits_to_u64(&tape, symbols);
            let output_symbol = tape.last().copied();
            if let Some(trace) = trace.as_mut() {
                trace.steps.push(TmTraceStep {
                    step: step + 1,
                    state,
                    head_before,
                    read,
                    next: trans.next,
                    write: trans.write,
                    move_dir: trans.move_dir,
                    head_after: head_before + 1,
                    tape: tape.clone(),
                });
            }
            return TmRunResult {
                output_value,
                output_symbol,
                halted: true,
                steps_taken: (step + 1) as u32,
                stop_reason: TmStopReason::Output,
                trace,
            };
        }

        let mut head_after = head_before;
        match trans.move_dir {
            super::TmMove::Left => {
                if head_after == 0 {
                    tape.insert(0, blank);
                    head_after = 0;
                } else {
                    head_after -= 1;
                }
            }
            super::TmMove::Stay => {}
            super::TmMove::Right => {
                if head_after + 1 < tape.len() {
                    head_after += 1;
                }
            }
        }

        if let Some(trace) = trace.as_mut() {
            trace.steps.push(TmTraceStep {
                step: step + 1,
                state,
                head_before,
                read,
                next: trans.next,
                write: trans.write,
                move_dir: trans.move_dir,
                head_after,
                tape: tape.clone(),
            });
        }

        head = head_after;
        state = trans.next;
        if state == 0 {
            return TmRunResult::terminated(
                (step + 1) as u32,
                TmStopReason::InvalidState,
                trace,
            );
        }
    }

    TmRunResult::terminated(max_steps, TmStopReason::MaxSteps, trace)
}

// ── TM output helper ─────────────────────────────────────────

/// Map a TM output symbol to a game action: 0 maps to Cooperate,
/// any non-zero symbol maps to Defect.
pub(crate) fn tm_action_from_output_symbol(symbol: u8) -> Action {
    if symbol == 0 {
        Action::Cooperate
    } else {
        Action::Defect
    }
}

// ── One-sided TM strategy ────────────────────────────────────

/// Strategy that runs a one-sided Turing machine each round to decide an action.
#[derive(Clone, Debug)]
pub struct OneSidedTmStrategy {
    id: String,
    symbols: u8,
    start_state: u16,
    blank: u8,
    max_steps_per_round: u32,
    transitions: Vec<super::TmTransition>,
    input_suffix: InputSuffix,
    last_halted: bool,
    stats: TmRunStats,
}

impl OneSidedTmStrategy {
    /// Construct from TM parameters and transition table.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        symbols: u8,
        start_state: u16,
        blank: u8,
        _fallback_symbol: u8,
        max_steps_per_round: u32,
        _input_mode: super::InputMode,
        _output_map: Vec<Action>,
        transitions: Vec<super::TmTransition>,
    ) -> Self {
        let symbols = symbols.max(2);
        let width = max_steps_per_round as usize + 1;
        Self {
            id: id.into(),
            symbols,
            start_state,
            blank,
            max_steps_per_round,
            transitions,
            input_suffix: InputSuffix::new(symbols, width.max(1)),
            last_halted: true,
            stats: TmRunStats::default(),
        }
    }

    /// Accumulated run statistics.
    pub fn stats(&self) -> &TmRunStats {
        &self.stats
    }

    fn action_from_output_symbol(&self, symbol: u8) -> Action {
        tm_action_from_output_symbol(symbol)
    }

    fn sync_input(&mut self, history: &History) {
        let len = history.len();
        if len < self.input_suffix.history_len || len > self.input_suffix.history_len + 1 {
            self.input_suffix.reset();
            for round in history.iter() {
                self.input_suffix.push_round(round);
            }
            self.input_suffix.history_len = len;
            return;
        }
        if len == self.input_suffix.history_len + 1 {
            if let Some(last) = history.last() {
                self.input_suffix.push_round(last);
            }
            self.input_suffix.history_len = len;
        }
    }

    fn record_round(
        &mut self,
        steps_taken: u32,
        halted: bool,
        output_event: bool,
        max_steps_hit: bool,
    ) {
        self.last_halted = halted;
        self.stats.rounds = self.stats.rounds.saturating_add(1);
        self.stats.steps = self.stats.steps.saturating_add(steps_taken as u64);
        if self.stats.rounds == 1 {
            self.stats.min_steps = steps_taken;
            self.stats.max_steps = steps_taken;
        } else {
            self.stats.min_steps = self.stats.min_steps.min(steps_taken);
            self.stats.max_steps = self.stats.max_steps.max(steps_taken);
        }
        if output_event {
            self.stats.output_events = self.stats.output_events.saturating_add(1);
        } else {
            self.stats.fallback = self.stats.fallback.saturating_add(1);
            if max_steps_hit {
                self.stats.max_steps_hits = self.stats.max_steps_hits.saturating_add(1);
            }
        }
    }
}

impl super::Strategy for OneSidedTmStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.input_suffix.reset();
        self.last_halted = true;
        self.stats = TmRunStats::default();
    }

    fn next_action(&mut self, history: &History, _player_a: bool) -> Action {
        if history.is_empty() {
            self.record_round(0, true, true, false);
            return Action::Cooperate;
        }

        self.sync_input(history);
        let input_digits = self.input_suffix.msd_digits();
        let run = run_one_sided_tm(
            &self.transitions,
            self.symbols,
            self.start_state,
            self.blank,
            &input_digits,
            self.max_steps_per_round,
            false,
        );

        self.record_round(
            run.steps_taken,
            run.halted,
            run.halted,
            matches!(run.stop_reason, TmStopReason::MaxSteps),
        );

        if let Some(symbol) = run.output_symbol {
            self.action_from_output_symbol(symbol)
        } else {
            Action::Defect
        }
    }

    fn last_halted(&self) -> bool {
        self.last_halted
    }

    fn tm_stats(&self) -> Option<&TmRunStats> {
        Some(&self.stats)
    }
}

// ── Input suffix (incremental history encoding) ──────────────

/// Incrementally tracks the base-`base` representation of the joint action
/// history as a suffix of width `width` least-significant digits.
///
/// Used by [`OneSidedTmStrategy`] and the halting filter to convert round
/// history into TM input without recomputing from scratch each round.
#[derive(Clone, Debug)]
pub(crate) struct InputSuffix {
    base: u8,
    width: usize,
    digits_le: Vec<u8>,
    prefix_nonzero: bool,
    pub(crate) history_len: usize,
}

impl InputSuffix {
    pub(crate) fn new(base: u8, width: usize) -> Self {
        Self {
            base: base.max(2),
            width: width.max(1),
            digits_le: vec![0],
            prefix_nonzero: false,
            history_len: 0,
        }
    }

    fn reset(&mut self) {
        self.digits_le.clear();
        self.digits_le.push(0);
        self.prefix_nonzero = false;
        self.history_len = 0;
    }

    fn push_round(&mut self, round: RoundRecord) {
        self.push_pair_bits(super::action_bit(round.a), super::action_bit(round.b));
    }

    pub(crate) fn push_pair_bits(&mut self, a_bit: u8, b_bit: u8) {
        let pair = ((a_bit.min(1) << 1) | b_bit.min(1)) as u16;
        self.mul_add(4, pair);
    }

    fn mul_add(&mut self, mul: u16, add: u16) {
        let base = self.base as u16;
        let mut carry = add;
        for digit in &mut self.digits_le {
            let value = (*digit as u16).saturating_mul(mul).saturating_add(carry);
            *digit = (value % base) as u8;
            carry = value / base;
        }
        while carry > 0 {
            if self.digits_le.len() < self.width {
                self.digits_le.push((carry % base) as u8);
                carry /= base;
            } else {
                self.prefix_nonzero = true;
                break;
            }
        }
        while self.digits_le.len() > self.width {
            let popped = self.digits_le.pop();
            if popped.unwrap_or(0) != 0 {
                self.prefix_nonzero = true;
            }
        }
        if self.prefix_nonzero {
            self.trim_most_significant_zeros_with_prefix();
        } else {
            self.trim_redundant_high_zeros();
        }
    }

    pub(crate) fn msd_digits(&self) -> Vec<u8> {
        if self.digits_le.is_empty() {
            return vec![0];
        }
        let mut out = self.digits_le.iter().rev().copied().collect::<Vec<_>>();
        if !self.prefix_nonzero {
            while out.len() > 1 && out.first() == Some(&0) {
                out.remove(0);
            }
        }
        if out.is_empty() {
            vec![0]
        } else {
            out
        }
    }

    fn trim_redundant_high_zeros(&mut self) {
        while self.digits_le.len() > 1 && self.digits_le.last() == Some(&0) {
            self.digits_le.pop();
        }
    }

    fn trim_most_significant_zeros_with_prefix(&mut self) {
        while self.digits_le.len() > self.width {
            self.digits_le.pop();
        }
        while self.digits_le.len() > 1 && self.digits_le.last() == Some(&0) {
            if self.digits_le.len() == self.width {
                break;
            }
            self.digits_le.pop();
        }
    }
}
