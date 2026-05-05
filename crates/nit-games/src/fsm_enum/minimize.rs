//! Behaviour-preserving minimisation via Hopcroft-style block refinement.

use std::collections::HashMap;

use super::canonical::canonicalize_raw_fsm;
use super::RawFsm;

pub(super) fn minimize_raw_fsm(raw: &RawFsm, start_state: usize) -> RawFsm {
    let machine = canonicalize_raw_fsm(raw, start_state);
    if machine.states() <= 1 || machine.actions == 0 {
        return machine;
    }

    let state_count = machine.states();
    let mut block_by_state = vec![0usize; state_count];
    let mut output_blocks: HashMap<usize, usize> = HashMap::new();
    for (state, slot) in block_by_state.iter_mut().enumerate() {
        let output = machine.outputs[state];
        let next = output_blocks.len();
        *slot = *output_blocks.entry(output).or_insert(next);
    }

    loop {
        let mut signature_blocks: HashMap<Vec<usize>, usize> = HashMap::new();
        let mut refined = vec![0usize; state_count];
        for (state, slot) in refined.iter_mut().enumerate() {
            let mut signature = Vec::with_capacity(machine.actions + 1);
            signature.push(machine.outputs[state]);
            for &next in machine.transitions[state].iter() {
                signature.push(block_by_state[next]);
            }
            let next = signature_blocks.len();
            *slot = *signature_blocks.entry(signature).or_insert(next);
        }
        if refined == block_by_state {
            break;
        }
        block_by_state = refined;
    }

    let block_count = block_by_state
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut representative = vec![usize::MAX; block_count];
    for (state, block) in block_by_state.iter().copied().enumerate() {
        if representative[block] == usize::MAX {
            representative[block] = state;
        }
    }

    let mut outputs = vec![0usize; block_count];
    let mut transitions = vec![vec![0usize; machine.actions]; block_count];
    for block in 0..block_count {
        let state = representative[block].min(state_count.saturating_sub(1));
        outputs[block] = machine.outputs[state];
        let row = &mut transitions[block];
        for (cell, &next) in row.iter_mut().zip(machine.transitions[state].iter()) {
            *cell = block_by_state[next];
        }
    }

    canonicalize_raw_fsm(
        &RawFsm {
            outputs,
            transitions,
            actions: machine.actions,
        },
        block_by_state[0],
    )
}
