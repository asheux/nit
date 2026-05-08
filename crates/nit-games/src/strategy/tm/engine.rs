//! Pure one-sided TM evaluation: stepping the tape, tracing, and halting.

use crate::strategy::math::{digits_in_base, digits_to_u64};
use crate::strategy::{TmMove, TmTransition};

use super::{TmRunResult, TmStopReason, TmTrace, TmTraceStep};

/// Cap the trace buffer at this many steps to keep traced runs bounded.
const MAX_TRACE_CAPACITY: u32 = 10_000;

/// States are 1-indexed externally; subtracting 1 gives the 0-based row.
fn transition_index(current_state: u16, read_symbol: u8, symbol_count: u8) -> usize {
    (current_state.saturating_sub(1) as usize)
        .saturating_mul(symbol_count as usize)
        .saturating_add(read_symbol as usize)
}

/// Move the head and grow the tape leftward with a blank when the head would
/// underflow position 0. Right-moves past the last cell are handled by the
/// caller as the output-halt path.
fn apply_head_movement(
    direction: TmMove,
    head_pos: usize,
    tape: &mut Vec<u8>,
    blank_symbol: u8,
) -> usize {
    match direction {
        TmMove::Left => {
            if head_pos == 0 {
                tape.insert(0, blank_symbol);
                0
            } else {
                head_pos - 1
            }
        }
        TmMove::Stay => head_pos,
        TmMove::Right => {
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
    transition: TmTransition,
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
    transitions: &[TmTransition],
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
/// rightmost). Halts with output when the head moves past the last cell.
pub fn run_one_sided_tm(
    transitions: &[TmTransition],
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
            initial_tape: tape.clone(),
            initial_head: head_pos,
            steps: Vec::with_capacity(max_steps.min(MAX_TRACE_CAPACITY) as usize),
            input_digits: initial_digits,
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

        // Output halt: the head walks off the right edge of the tape.
        if matches!(transition.move_dir, TmMove::Right) && head_before + 1 == tape.len() {
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
