//! Canonical FSM forms via BFS state-renumbering plus the cached enumeration
//! of distinct canonical representatives.

mod odometer;

use std::collections::HashMap;

use rayon::prelude::*;

use crate::strategy::{
    action_bit, decode_notebook_index_digits, symbol_to_action, validate_decode_params,
};

use super::cache::{
    canonical_fsm_cache, clone_cached_vec_result, insert_min_index, merge_min_index_maps,
};
use super::{FsmDefinition, RawFsm};

pub use odometer::enumerate_fsms;

const KEY_DIGIT_MAX: usize = u16::MAX as usize;

/// BFS-renumber states from `def.start_state` so the start becomes state 0
/// and unreachable states are dropped. Two FSMs that differ only in state
/// numbering normalise to the same canonical form.
pub fn canonicalize_fsm(def: &FsmDefinition) -> FsmDefinition {
    if def.num_states == 0 || def.outputs.is_empty() {
        return def.clone();
    }
    let entry = def.start_state.min(def.num_states.saturating_sub(1));
    let raw = RawFsm {
        outputs: def
            .outputs
            .iter()
            .map(|&a| action_bit(a) as usize)
            .collect(),
        transitions: def.transitions.clone(),
        actions: def.input_mode.alphabet_size(),
    };
    let folded = canonicalize_raw_fsm(&raw, entry);
    FsmDefinition {
        num_states: folded.states(),
        start_state: 0,
        outputs: folded
            .outputs
            .iter()
            .map(|&d| symbol_to_action(d as u8))
            .collect(),
        input_mode: def.input_mode,
        transitions: folded.transitions,
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
    let entry = start_state.min(raw.states().saturating_sub(1));
    let mut walk = BfsWalk::seeded(raw, entry);
    walk.run();
    walk.into_canonical()
}

/// Single-pass BFS that renumbers reachable states (entry → 0, then in
/// discovery order) and assembles the canonical FSM in one shot. Holding
/// `discovery` and `new_label` together lets the second-pass relabel reuse
/// the BFS frontier instead of re-walking the source transitions.
struct BfsWalk<'src> {
    source: &'src RawFsm,
    new_label: Vec<Option<usize>>,
    discovery: Vec<usize>,
    cursor: usize,
    state_ceiling: usize,
}

impl<'src> BfsWalk<'src> {
    fn seeded(source: &'src RawFsm, entry: usize) -> Self {
        let mut new_label = vec![None; source.states()];
        new_label[entry] = Some(0);
        Self {
            source,
            new_label,
            discovery: vec![entry],
            cursor: 0,
            state_ceiling: source.states().saturating_sub(1),
        }
    }

    fn run(&mut self) {
        while self.cursor < self.discovery.len() {
            let here = self.discovery[self.cursor];
            self.cursor += 1;
            for input in 0..self.source.actions {
                self.visit(self.successor(here, input));
            }
        }
    }

    fn visit(&mut self, target: usize) {
        if self.new_label[target].is_some() {
            return;
        }
        self.new_label[target] = Some(self.discovery.len());
        self.discovery.push(target);
    }

    fn successor(&self, state: usize, input: usize) -> usize {
        self.source
            .transitions
            .get(state)
            .and_then(|row| row.get(input))
            .copied()
            .unwrap_or(state)
            .min(self.state_ceiling)
    }

    fn into_canonical(self) -> RawFsm {
        let outputs = self
            .discovery
            .iter()
            .map(|&s| self.source.outputs.get(s).copied().unwrap_or(0))
            .collect();
        let transitions = self
            .discovery
            .iter()
            .map(|&original| self.relabelled_row(original))
            .collect();
        RawFsm {
            outputs,
            transitions,
            actions: self.source.actions,
        }
    }

    fn relabelled_row(&self, original: usize) -> Vec<usize> {
        (0..self.source.actions)
            .map(|input| self.new_label[self.successor(original, input)].unwrap_or(0))
            .collect()
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
    if raw.actions > KEY_DIGIT_MAX || raw.states() > KEY_DIGIT_MAX {
        return Err(format!(
            "fsm shape (states={}, actions={}) exceeds key width {KEY_DIGIT_MAX}",
            raw.states(),
            raw.actions,
        ));
    }
    raw.outputs
        .iter()
        .copied()
        .chain(raw.transitions.iter().flat_map(|row| row.iter().copied()))
        .map(narrow_digit)
        .collect()
}

fn narrow_digit(digit: usize) -> Result<u16, String> {
    if digit > KEY_DIGIT_MAX {
        return Err("fsm key digit exceeds supported width".to_string());
    }
    Ok(digit as u16)
}

pub(super) fn fsm_index_limit(states: usize, actions: usize) -> Result<u64, String> {
    let total = crate::strategy::fsm_count(states, actions)
        .ok_or_else(|| "fsm index space overflows u128 for this (states, actions)".to_string())?;
    match total {
        0 => Ok(0),
        upper if upper > u64::MAX as u128 => Err("fsm index space exceeds u64 range".to_string()),
        upper => Ok(upper as u64),
    }
}

fn canonical_fsm_indices_uncached(states: usize, actions: usize) -> Result<Vec<u64>, String> {
    let upper = fsm_index_limit(states, actions)?;
    if upper == 0 {
        return Ok(Vec::new());
    }
    let signature_for = |candidate: u64| -> Result<Vec<u16>, String> {
        let raw = decode_fsm_notebook_index_raw(candidate, states, actions)?;
        raw_fsm_key(&canonicalize_raw_fsm(&raw, 0))
    };
    let earliest_by_key = (0..upper)
        .into_par_iter()
        .try_fold(HashMap::new, |mut shard, candidate| {
            insert_min_index(&mut shard, signature_for(candidate)?, candidate);
            Ok::<_, String>(shard)
        })
        .try_reduce(HashMap::new, |mut accum, shard| {
            merge_min_index_maps(&mut accum, shard);
            Ok::<_, String>(accum)
        })?;
    let mut representatives = earliest_by_key.into_values().collect::<Vec<_>>();
    representatives.sort_unstable();
    Ok(representatives)
}
