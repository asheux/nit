//! Behavioural-equivalence grouping via input-trace signatures (WNBM) and
//! canonical-minimised keys (Moorem).

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

/// Group canonical indices by behavioural equivalence (default WNBM mode).
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

/// One canonical index per distinct behaviour (default WNBM mode).
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
        let mut code = sequence_idx;
        for pos in (0..steps).rev() {
            let digit = code % raw.actions;
            code /= raw.actions;
            digits[pos] = raw.actions - 1 - digit;
        }

        let mut state = 0usize;
        for &input in &digits {
            let next = raw
                .transitions
                .get(state)
                .and_then(|row| row.get(input))
                .copied()
                .unwrap_or(state)
                .min(raw.states().saturating_sub(1));
            let out = raw.outputs.get(next).copied().unwrap_or(0);
            if out > u16::MAX as usize {
                return Err(
                    "fsm behavior trace output digit exceeds supported key width".to_string(),
                );
            }
            signature.push(out as u16);
            state = next;
        }
    }

    Ok(signature)
}
