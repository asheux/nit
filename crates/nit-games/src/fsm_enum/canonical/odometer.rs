//! Brute-force enumeration of every `(outputs, transitions)` pair for a given
//! state count, optionally collapsing each canonical equivalence class into a
//! single emission. Used by the strategy generator to seed the FSM corpus.

use std::collections::HashSet;

use crate::game::Action;
use crate::strategy::InputMode;

use super::{canonicalize_fsm, FsmDefinition};

/// Iterate every Action × transition combination for the given shape and emit
/// each FSM (or one canonical representative per equivalence class when
/// `canonical` is true). Returns the emitted count, capped by `limit`.
pub fn enumerate_fsms<F>(
    num_states: usize,
    input_mode: InputMode,
    limit: Option<usize>,
    canonical: bool,
    mut emit: F,
) -> usize
where
    F: FnMut(FsmDefinition),
{
    // 63 bits because the output mask is u64 and we shift `1u64 << num_states`.
    if num_states == 0 || num_states >= 63 {
        return 0;
    }
    let alphabet = input_mode.alphabet_size();
    let output_variants = 1u64 << num_states;
    let mut count = 0usize;
    let mut seen = HashSet::new();

    for mask in 0..output_variants {
        let outputs = decode_output_mask(mask, num_states);

        let mut transitions = vec![vec![0usize; alphabet]; num_states];
        let mut done = false;
        while !done {
            let spec = FsmDefinition {
                num_states,
                start_state: 0,
                outputs: outputs.clone(),
                input_mode,
                transitions: transitions.clone(),
            };

            if canonical {
                let output_spec = canonicalize_fsm(&spec);
                let key = output_spec.stable_key();
                if !seen.insert(key) {
                    done = !increment_odometer(&mut transitions, num_states);
                    continue;
                }
                emit(output_spec);
            } else {
                emit(spec);
            }
            count += 1;
            if let Some(limit) = limit {
                if count >= limit {
                    return count;
                }
            }

            done = !increment_odometer(&mut transitions, num_states);
        }
    }

    count
}

fn decode_output_mask(mask: u64, num_states: usize) -> Vec<Action> {
    (0..num_states)
        .map(|state| {
            if (mask >> state) & 1 == 0 {
                Action::Cooperate
            } else {
                Action::Defect
            }
        })
        .collect()
}

/// Mixed-radix odometer over the flattened transition table. Returns false
/// when every digit has wrapped back to zero (enumeration exhausted).
fn increment_odometer(transitions: &mut [Vec<usize>], num_states: usize) -> bool {
    for row in transitions.iter_mut() {
        for cell in row.iter_mut() {
            if *cell + 1 < num_states {
                *cell += 1;
                return true;
            }
            *cell = 0;
        }
    }
    false
}
