//! One-sided Turing machine (TM) strategy implementation.

use super::math::{digits_in_base, digits_to_u64};
use crate::game::Action;
use crate::history::{History, RoundRecord};

const MAX_TRACE_CAPACITY: u32 = 10_000;

// ── TM run statistics ────────────────────────────────────────

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
    fn update_step_range(&mut self, min: u32, max: u32) {
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

// ── TM trace types ───────────────────────────────────────────

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

/// States are 1-indexed externally; subtracting 1 gives the 0-based row.
fn transition_index(current_state: u16, read_symbol: u8, symbol_count: u8) -> usize {
    (current_state.saturating_sub(1) as usize)
        .saturating_mul(symbol_count as usize)
        .saturating_add(read_symbol as usize)
}

/// Move the head, expanding the tape leftward with a blank when the head
/// would underflow position 0.
fn apply_head_movement(
    direction: super::TmMove,
    head_pos: usize,
    tape: &mut Vec<u8>,
    blank_symbol: u8,
) -> usize {
    match direction {
        super::TmMove::Left => {
            if head_pos == 0 {
                tape.insert(0, blank_symbol);
                0
            } else {
                head_pos - 1
            }
        }
        super::TmMove::Stay => head_pos,
        super::TmMove::Right => {
            if head_pos + 1 < tape.len() {
                head_pos + 1
            } else {
                head_pos
            }
        }
    }
}

struct StepSnapshot {
    step_number: usize,
    current_state: u16,
    head_before: usize,
    read_symbol: u8,
    transition: super::TmTransition,
    head_after: usize,
}

fn record_trace_step(trace: &mut Option<TmTrace>, snap: &StepSnapshot, tape: &[u8]) {
    let Some(active_trace) = trace.as_mut() else {
        return;
    };
    active_trace.steps.push(TmTraceStep {
        step: snap.step_number,
        state: snap.current_state,
        head_before: snap.head_before,
        read: snap.read_symbol,
        next: snap.transition.next,
        write: snap.transition.write,
        move_dir: snap.transition.move_dir,
        head_after: snap.head_after,
        tape: tape.to_vec(),
    });
}

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

/// Run a one-sided TM on explicit input digits (MSD first, head starts
/// rightmost). Halts with output when head moves past the last cell.
pub fn run_one_sided_tm(
    transitions: &[super::TmTransition],
    symbol_count: u8,
    start_state: u16,
    blank_symbol: u8,
    input_digits: &[u8],
    max_steps: u32,
    with_trace: bool,
) -> TmRunResult {
    let symbol_count = symbol_count.max(1);
    let initial_digits = if input_digits.is_empty() {
        vec![0]
    } else {
        input_digits.to_vec()
    };
    let mut tape = initial_digits.clone();
    let mut head_pos = tape.len().saturating_sub(1);
    let mut current_state = start_state;

    let mut trace = if with_trace {
        Some(TmTrace {
            input_digits: initial_digits.clone(),
            initial_tape: tape.clone(),
            initial_head: head_pos,
            steps: Vec::with_capacity(max_steps.min(MAX_TRACE_CAPACITY) as usize),
        })
    } else {
        None
    };

    if max_steps == 0 {
        return TmRunResult::terminated(0, TmStopReason::MaxSteps, trace);
    }
    if current_state == 0 {
        return TmRunResult::terminated(0, TmStopReason::InvalidState, trace);
    }

    for step_idx in 0..(max_steps as usize) {
        let step_number = step_idx + 1;
        let head_before = head_pos;
        let read_symbol = tape.get(head_before).copied().unwrap_or(blank_symbol);

        let table_index = transition_index(current_state, read_symbol, symbol_count);
        let Some(transition) = transitions.get(table_index).copied() else {
            return TmRunResult::terminated(
                step_number as u32,
                TmStopReason::MissingTransition,
                trace,
            );
        };

        if let Some(cell) = tape.get_mut(head_before) {
            *cell = transition.write;
        }

        // Output halt: head moves right past the last tape cell.
        if matches!(transition.move_dir, super::TmMove::Right) && head_before + 1 == tape.len() {
            let snap = StepSnapshot {
                step_number,
                current_state,
                head_before,
                read_symbol,
                transition,
                head_after: head_before + 1,
            };
            record_trace_step(&mut trace, &snap, &tape);
            return TmRunResult {
                output_value: digits_to_u64(&tape, symbol_count),
                output_symbol: tape.last().copied(),
                halted: true,
                steps_taken: step_number as u32,
                stop_reason: TmStopReason::Output,
                trace,
            };
        }

        let head_after =
            apply_head_movement(transition.move_dir, head_before, &mut tape, blank_symbol);

        let snap = StepSnapshot {
            step_number,
            current_state,
            head_before,
            read_symbol,
            transition,
            head_after,
        };
        record_trace_step(&mut trace, &snap, &tape);

        head_pos = head_after;
        current_state = transition.next;
        if current_state == 0 {
            return TmRunResult::terminated(step_number as u32, TmStopReason::InvalidState, trace);
        }
    }

    TmRunResult::terminated(max_steps, TmStopReason::MaxSteps, trace)
}

// ── One-sided TM strategy ────────────────────────────────────

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
    pub fn new(
        id: impl Into<String>,
        symbols: u8,
        start_state: u16,
        blank: u8,
        max_steps_per_round: u32,
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

    pub fn stats(&self) -> &TmRunStats {
        &self.stats
    }

    /// Bring the input suffix up to date with the current history.
    ///
    /// When exactly one round was appended since the last sync, only that
    /// round is pushed incrementally. Otherwise the suffix is rebuilt from
    /// scratch (handles resets or skipped rounds).
    fn sync_input(&mut self, history: &History) {
        let len = history.len();
        let prev = self.input_suffix.history_len;

        if len == prev + 1 {
            // Fast path: exactly one new round appended.
            if let Some(last) = history.last() {
                self.input_suffix.push_round(last);
            }
        } else if len != prev {
            // Full rebuild: history was reset or jumped by more than one round.
            self.input_suffix.reset();
            for round in history.iter() {
                self.input_suffix.push_round(round);
            }
        }

        self.input_suffix.history_len = len;
    }

    fn record_round(
        &mut self,
        steps_taken: u32,
        halted: bool,
        output_event: bool,
        max_steps_hit: bool,
    ) {
        self.last_halted = halted;
        self.stats.update_step_range(steps_taken, steps_taken);
        self.stats.rounds = self.stats.rounds.saturating_add(1);
        self.stats.steps = self.stats.steps.saturating_add(steps_taken as u64);
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
            super::symbol_to_action(symbol)
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
        let action_pair = ((a_bit << 1) | b_bit) as u16;
        self.mul_add(4, action_pair);
    }

    fn mul_add(&mut self, multiplier: u16, addend: u16) {
        let radix = self.base as u16;
        let mut overflow = addend;
        for slot in &mut self.digits_le {
            let product = (*slot as u16)
                .saturating_mul(multiplier)
                .saturating_add(overflow);
            *slot = (product % radix) as u8;
            overflow = product / radix;
        }
        self.propagate_carry(overflow, radix);
        self.truncate_to_width();
        if self.prefix_nonzero {
            self.trim_most_significant_zeros_with_prefix();
        } else {
            self.trim_redundant_high_zeros();
        }
    }

    /// Extend `digits_le` with carry digits until the carry is exhausted or
    /// the width limit is reached (setting the prefix-overflow flag).
    fn propagate_carry(&mut self, mut remaining: u16, radix: u16) {
        while remaining > 0 {
            if self.digits_le.len() < self.width {
                self.digits_le.push((remaining % radix) as u8);
                remaining /= radix;
            } else {
                self.prefix_nonzero = true;
                return;
            }
        }
    }

    /// Drop digits beyond `self.width`, flagging prefix overflow if any
    /// non-zero digit is discarded.
    fn truncate_to_width(&mut self) {
        if self.digits_le.len() > self.width {
            if self.digits_le[self.width..].iter().any(|&d| d != 0) {
                self.prefix_nonzero = true;
            }
            self.digits_le.truncate(self.width);
        }
    }

    /// Return the current digits in most-significant-digit-first order.
    ///
    /// Strips leading zeros unless the prefix overflow flag is set, in
    /// which case the full width is preserved to maintain alignment.
    pub(crate) fn msd_digits(&self) -> Vec<u8> {
        if self.digits_le.is_empty() {
            return vec![0];
        }
        let msd_iter = self.digits_le.iter().rev().copied();
        if self.prefix_nonzero {
            msd_iter.collect()
        } else {
            let result: Vec<u8> = msd_iter.skip_while(|&d| d == 0).collect();
            if result.is_empty() {
                vec![0]
            } else {
                result
            }
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
