//! Behavioural-equivalence grouping. Two FSMs are equivalent iff they produce
//! the same output sequence for every input sequence — the trace signature
//! (WNBM mode) samples this at fixed depth, while Moorem normalises through
//! Hopcroft minimisation. Both produce a `Vec<u16>` key suitable for hashing.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::config::FsmGroupingMode;
use crate::strategy::math::checked_pow_u128;

use super::cache::{
    behavior_rep_cache, clone_cached_vec_result, insert_min_index, merge_min_index_maps,
};
use super::canonical::{canonical_fsm_indices, decode_fsm_notebook_index_raw, raw_fsm_key};
use super::minimize::minimize_raw_fsm;
use super::RawFsm;

const NOTEBOOK_BEHAVIOR_TRACE_STEPS: usize = 12;

pub fn group_canonical_fsm_indices_by_behavior(
    states: usize,
    actions: usize,
) -> Result<Vec<Vec<u64>>, String> {
    group_canonical_fsm_indices_by_behavior_with_mode(states, actions, FsmGroupingMode::Wnbm)
}

pub fn group_canonical_fsm_indices_by_behavior_with_mode(
    states: usize,
    actions: usize,
    mode: FsmGroupingMode,
) -> Result<Vec<Vec<u64>>, String> {
    let canonical = canonical_fsm_indices(states, actions)?;
    let mut group_index_by_key: HashMap<Vec<u16>, usize> = HashMap::new();
    let mut groups: Vec<Vec<u64>> = Vec::new();

    for idx in canonical {
        let raw = decode_fsm_notebook_index_raw(idx, states, actions)?;
        let key = grouping_key(&raw, mode)?;
        if let Some(group_idx) = group_index_by_key.get(&key).copied() {
            groups[group_idx].push(idx);
        } else {
            group_index_by_key.insert(key, groups.len());
            groups.push(vec![idx]);
        }
    }

    Ok(groups)
}

pub fn unique_fsm_behavior_representatives(
    states: usize,
    actions: usize,
) -> Result<Vec<u64>, String> {
    unique_fsm_behavior_representatives_with_mode(states, actions, FsmGroupingMode::Wnbm)
}

pub fn unique_fsm_behavior_representatives_with_mode(
    states: usize,
    actions: usize,
    mode: FsmGroupingMode,
) -> Result<Vec<u64>, String> {
    clone_cached_vec_result(
        behavior_rep_cache(),
        (states, actions, mode),
        |&(states, actions, mode)| {
            unique_fsm_behavior_representatives_with_mode_uncached(states, actions, mode)
        },
    )
}

fn unique_fsm_behavior_representatives_with_mode_uncached(
    states: usize,
    actions: usize,
    mode: FsmGroupingMode,
) -> Result<Vec<u64>, String> {
    let canonical = canonical_fsm_indices(states, actions)?;
    let reps_by_key = canonical
        .into_par_iter()
        .try_fold(HashMap::new, |mut local, idx| {
            let raw = decode_fsm_notebook_index_raw(idx, states, actions)?;
            let key = grouping_key(&raw, mode)?;
            insert_min_index(&mut local, key, idx);
            Ok::<_, String>(local)
        })
        .try_reduce(HashMap::new, |mut left, right| {
            merge_min_index_maps(&mut left, right);
            Ok::<_, String>(left)
        })?;
    let mut reps = reps_by_key.into_values().collect::<Vec<_>>();
    reps.sort_unstable();
    Ok(reps)
}

fn grouping_key(raw: &RawFsm, mode: FsmGroupingMode) -> Result<Vec<u16>, String> {
    match mode {
        FsmGroupingMode::Wnbm => behavior_trace_signature(raw, NOTEBOOK_BEHAVIOR_TRACE_STEPS),
        FsmGroupingMode::Moorem => raw_fsm_key(&minimize_raw_fsm(raw, 0)),
    }
}

/// Concatenate the FSM's outputs across every input sequence of length
/// `steps`. Two FSMs share this signature iff they're externally
/// indistinguishable up to `steps` inputs.
fn behavior_trace_signature(raw: &RawFsm, steps: usize) -> Result<Vec<u16>, String> {
    if raw.states() == 0 || raw.actions == 0 || steps == 0 {
        return Ok(Vec::new());
    }
    let sequence_count = checked_pow_u128(raw.actions as u128, steps as u32)
        .ok_or_else(|| "fsm behavior trace space overflows u128".to_string())?;
    if sequence_count > usize::MAX as u128 {
        return Err("fsm behavior trace space exceeds usize".to_string());
    }
    let sequence_count = sequence_count as usize;
    let capacity = sequence_count
        .checked_mul(steps)
        .ok_or_else(|| "fsm behavior trace signature capacity overflow".to_string())?;
    let mut signature = Vec::with_capacity(capacity);
    let mut digits = vec![0usize; steps];

    for sequence_idx in 0..sequence_count {
        decode_input_sequence(sequence_idx, raw.actions, &mut digits);
        run_and_record(raw, &digits, &mut signature)?;
    }

    Ok(signature)
}

/// Decode a sequence index into per-step input digits. The reversal
/// `actions - 1 - digit` keeps the legacy ordering used by the cached
/// keys in production.
fn decode_input_sequence(mut code: usize, actions: usize, digits: &mut [usize]) {
    let steps = digits.len();
    for pos in (0..steps).rev() {
        let digit = code % actions;
        code /= actions;
        digits[pos] = actions - 1 - digit;
    }
}

fn run_and_record(raw: &RawFsm, digits: &[usize], signature: &mut Vec<u16>) -> Result<(), String> {
    let max_state = raw.states().saturating_sub(1);
    let mut state = 0usize;
    for &input in digits {
        let next = raw
            .transitions
            .get(state)
            .and_then(|row| row.get(input))
            .copied()
            .unwrap_or(state)
            .min(max_state);
        let out = raw.outputs.get(next).copied().unwrap_or(0);
        if out > u16::MAX as usize {
            return Err("fsm behavior trace output digit exceeds supported key width".to_string());
        }
        signature.push(out as u16);
        state = next;
    }
    Ok(())
}
