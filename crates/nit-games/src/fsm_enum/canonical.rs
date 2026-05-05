//! Canonical FSM forms via BFS state-renumbering and the cached enumeration of
//! distinct canonical representatives.

use std::collections::{HashMap, VecDeque};

use rayon::prelude::*;

use crate::game::Action;
use crate::strategy::{
    action_bit, decode_notebook_index_digits, symbol_to_action, validate_decode_params,
};

use super::cache::{
    canonical_fsm_cache, clone_cached_vec_result, insert_min_index, merge_min_index_maps,
};
use super::{FsmDefinition, RawFsm};

/// BFS-renumber states from `def.start_state` so the start becomes state 0
/// and unreachable states are dropped. Two FSMs that differ only in state
/// numbering normalise to the same canonical form.
pub fn canonicalize_fsm(def: &FsmDefinition) -> FsmDefinition {
    if def.num_states == 0 || def.outputs.is_empty() {
        return def.clone();
    }
    let start = def.start_state.min(def.num_states.saturating_sub(1));
    let raw = RawFsm {
        outputs: def
            .outputs
            .iter()
            .map(|&a| action_bit(a) as usize)
            .collect(),
        transitions: def.transitions.clone(),
        actions: def.input_mode.alphabet_size(),
    };
    let canonical = canonicalize_raw_fsm(&raw, start);
    FsmDefinition {
        num_states: canonical.states(),
        start_state: 0,
        outputs: canonical
            .outputs
            .iter()
            .map(|&idx| symbol_to_action(idx as u8))
            .collect(),
        input_mode: def.input_mode,
        transitions: canonical.transitions,
    }
}

/// Sorted canonical representative indices for `(states, actions)`. Cached.
pub fn canonical_fsm_indices(states: usize, actions: usize) -> Result<Vec<u64>, String> {
    clone_cached_vec_result(
        canonical_fsm_cache(),
        (states, actions),
        |&(states, actions)| canonical_fsm_indices_uncached(states, actions),
    )
}

pub(super) fn canonicalize_raw_fsm(raw: &RawFsm, start_state: usize) -> RawFsm {
    if raw.states() == 0 {
        return raw.clone();
    }
    let start = start_state.min(raw.states().saturating_sub(1));
    let mut state_map = vec![None; raw.states()];
    let mut order = Vec::with_capacity(raw.states());
    let mut queue = VecDeque::new();
    let mut next_id = 1usize;
    state_map[start] = Some(0usize);
    queue.push_back(start);

    while let Some(state) = queue.pop_front() {
        order.push(state);
        let row = raw.transitions.get(state);
        for input in 0..raw.actions {
            let next = row
                .and_then(|r| r.get(input))
                .copied()
                .unwrap_or(state)
                .min(raw.states().saturating_sub(1));
            if state_map[next].is_none() {
                state_map[next] = Some(next_id);
                next_id += 1;
                queue.push_back(next);
            }
        }
    }

    let mut outputs = Vec::with_capacity(order.len());
    let mut transitions = Vec::with_capacity(order.len());
    for &state in &order {
        outputs.push(raw.outputs.get(state).copied().unwrap_or(0));
        let mut row = Vec::with_capacity(raw.actions);
        for input in 0..raw.actions {
            let next = raw
                .transitions
                .get(state)
                .and_then(|r| r.get(input))
                .copied()
                .unwrap_or(state)
                .min(raw.states().saturating_sub(1));
            row.push(state_map[next].unwrap_or(0));
        }
        transitions.push(row);
    }

    RawFsm {
        outputs,
        transitions,
        actions: raw.actions,
    }
}

pub(super) fn decode_fsm_notebook_index_raw(
    index: u64,
    states: usize,
    actions: usize,
) -> Result<RawFsm, String> {
    validate_decode_params(index, states, actions)?;
    let (outputs, transitions) = decode_notebook_index_digits(index, states, actions)?;
    Ok(RawFsm {
        outputs,
        transitions,
        actions,
    })
}

pub(super) fn raw_fsm_key(raw: &RawFsm) -> Result<Vec<u16>, String> {
    if raw.actions > u16::MAX as usize {
        return Err(format!(
            "fsm actions {} exceed supported key width {}",
            raw.actions,
            u16::MAX
        ));
    }
    if raw.states() > u16::MAX as usize {
        return Err(format!(
            "fsm states {} exceed supported key width {}",
            raw.states(),
            u16::MAX
        ));
    }
    let mut key = Vec::with_capacity(raw.states().saturating_mul(raw.actions + 1));
    for output in &raw.outputs {
        if *output > u16::MAX as usize {
            return Err("fsm output digit exceeds supported key width".to_string());
        }
        key.push(*output as u16);
    }
    for row in &raw.transitions {
        for next in row {
            if *next > u16::MAX as usize {
                return Err("fsm transition target exceeds supported key width".to_string());
            }
            key.push(*next as u16);
        }
    }
    Ok(key)
}

pub(super) fn fsm_index_limit(states: usize, actions: usize) -> Result<u64, String> {
    let Some(count) = crate::strategy::fsm_count(states, actions) else {
        return Err("fsm index space overflows u128 for this (states, actions)".to_string());
    };
    if count == 0 {
        return Ok(0);
    }
    if count > u64::MAX as u128 {
        return Err("fsm index space exceeds u64 range".to_string());
    }
    Ok(count as u64)
}

fn canonical_fsm_indices_uncached(states: usize, actions: usize) -> Result<Vec<u64>, String> {
    let limit = fsm_index_limit(states, actions)?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let first_by_key = (0..limit)
        .into_par_iter()
        .try_fold(HashMap::new, |mut local, idx| {
            let raw = decode_fsm_notebook_index_raw(idx, states, actions)?;
            let canonical = canonicalize_raw_fsm(&raw, 0);
            let key = raw_fsm_key(&canonical)?;
            insert_min_index(&mut local, key, idx);
            Ok::<_, String>(local)
        })
        .try_reduce(HashMap::new, |mut left, right| {
            merge_min_index_maps(&mut left, right);
            Ok::<_, String>(left)
        })?;
    let mut canonical = first_by_key.into_values().collect::<Vec<_>>();
    canonical.sort_unstable();
    Ok(canonical)
}

/// Iterate every Action × transition combination for the given shape and emit
/// each FSM (or one canonical representative per equivalence class when
/// `canonical` is true). Returns the emitted count, capped by `limit`.
pub fn enumerate_fsms<F>(
    num_states: usize,
    input_mode: crate::strategy::InputMode,
    limit: Option<usize>,
    canonical: bool,
    mut emit: F,
) -> usize
where
    F: FnMut(FsmDefinition),
{
    if num_states == 0 || num_states >= 63 {
        return 0;
    }
    let alphabet = input_mode.alphabet_size();
    let output_variants = 1u64 << num_states;
    let mut count = 0usize;
    let mut seen = std::collections::HashSet::new();

    for mask in 0..output_variants {
        let mut outputs = Vec::with_capacity(num_states);
        for state in 0..num_states {
            let bit = (mask >> state) & 1;
            outputs.push(if bit == 0 {
                Action::Cooperate
            } else {
                Action::Defect
            });
        }

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

// Mixed-radix odometer over the flattened transition table. Returns false when
// every digit has wrapped back to zero (i.e. the enumeration is exhausted).
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
