//! Enumeration, canonicalisation, and behavioural grouping of FSM strategies.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::sync::{Mutex, OnceLock};

use rayon::prelude::*;

use crate::config::{FsmGroupingMode, StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::math::checked_pow_u128;
use crate::strategy::{
    action_bit, decode_notebook_index_digits, symbol_to_action, validate_decode_params, InputMode,
};
use nit_utils::hashing::stable_hash_bytes;

#[derive(Clone, Debug)]
pub struct FsmDefinition {
    pub num_states: usize,
    pub start_state: usize,
    pub outputs: Vec<Action>,
    pub input_mode: InputMode,
    pub transitions: Vec<Vec<usize>>,
}

impl FsmDefinition {
    pub fn to_spec(&self, id: String) -> StrategySpec {
        StrategySpec {
            id,
            name: None,
            kind: StrategySpecKind::Fsm {
                num_states: self.num_states,
                start_state: self.start_state,
                outputs: self.outputs.clone(),
                input_mode: Some(self.input_mode),
                transitions: self.transitions.clone(),
                index: None,
            },
        }
    }

    pub fn stable_key(&self) -> String {
        let mut out = String::new();
        out.push_str("mode=");
        out.push_str(match self.input_mode {
            InputMode::OpponentLastAction => "opp",
            InputMode::SelfLastAction => "self",
            InputMode::JointLastAction => "joint",
        });
        out.push_str(";states=");
        out.push_str(&self.num_states.to_string());
        out.push_str(";start=");
        out.push_str(&self.start_state.to_string());
        out.push_str(";outputs=");
        for action in &self.outputs {
            out.push(action.as_char());
        }
        out.push_str(";transitions=");
        for (row_idx, row) in self.transitions.iter().enumerate() {
            if row_idx > 0 {
                out.push('|');
            }
            for (col_idx, next) in row.iter().enumerate() {
                if col_idx > 0 {
                    out.push(',');
                }
                out.push_str(&next.to_string());
            }
        }
        out
    }

    pub fn stable_hash(&self) -> u64 {
        stable_hash_bytes(self.stable_key().as_bytes())
    }
}

/// Canonicalise an FSM by BFS-renumbering states from the start state.
///
/// Unreachable states are discarded and the start state becomes state 0. Two
/// FSMs that differ only in state numbering will produce identical canonical
/// forms. Delegates to the shared BFS implementation via `canonicalize_raw_fsm`.
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

/// Advance a mixed-radix odometer over the flattened transition table.
/// Returns `true` if the increment succeeded, `false` when all combinations
/// have been exhausted (i.e. every digit has wrapped back to zero).
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

/// Enumerate all FSM strategies, calling `emit` for each unique machine.
/// When `canonical` is true, duplicates under canonical equivalence are
/// suppressed. Returns the total emitted count.
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
    if num_states == 0 {
        return 0;
    }
    let alphabet = input_mode.alphabet_size();
    if num_states >= 63 {
        return 0;
    }
    let output_variants = 1u64 << num_states;
    let mut count = 0usize;
    let mut seen = HashSet::new();

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

/// Internal FSM representation with outputs as plain action indices.
#[derive(Clone, Debug)]
struct RawFsm {
    outputs: Vec<usize>,
    transitions: Vec<Vec<usize>>,
    actions: usize,
}

impl RawFsm {
    fn states(&self) -> usize {
        self.outputs.len()
    }
}

type CanonicalFsmCache = HashMap<(usize, usize), Result<Vec<u64>, String>>;
type BehaviorRepCache = HashMap<(usize, usize, FsmGroupingMode), Result<Vec<u64>, String>>;

fn canonical_fsm_cache() -> &'static Mutex<CanonicalFsmCache> {
    static CACHE: OnceLock<Mutex<CanonicalFsmCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn behavior_rep_cache() -> &'static Mutex<BehaviorRepCache> {
    static CACHE: OnceLock<Mutex<BehaviorRepCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn clone_cached_vec_result<K>(
    cache: &Mutex<HashMap<K, Result<Vec<u64>, String>>>,
    key: K,
    compute: impl FnOnce(&K) -> Result<Vec<u64>, String>,
) -> Result<Vec<u64>, String>
where
    K: Eq + Hash + Clone,
{
    if let Some(cached) = cache
        .lock()
        .expect("fsm cache lock poisoned")
        .get(&key)
        .cloned()
    {
        return cached;
    }

    let computed = compute(&key);
    cache
        .lock()
        .expect("fsm cache lock poisoned")
        .insert(key, computed.clone());
    computed
}

fn insert_min_index(map: &mut HashMap<Vec<u16>, u64>, key: Vec<u16>, idx: u64) {
    map.entry(key)
        .and_modify(|existing| *existing = (*existing).min(idx))
        .or_insert(idx);
}

fn merge_min_index_maps(left: &mut HashMap<Vec<u16>, u64>, right: HashMap<Vec<u16>, u64>) {
    for (key, idx) in right {
        insert_min_index(left, key, idx);
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
        let key = match mode {
            FsmGroupingMode::Wnbm => behavior_trace_signature(&raw, NOTEBOOK_BEHAVIOR_TRACE_STEPS)?,
            FsmGroupingMode::Moorem => raw_fsm_key(&minimize_raw_fsm(&raw, 0))?,
        };
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

/// One canonical index per distinct behaviour, with explicit mode. Cached.
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

fn fsm_index_limit(states: usize, actions: usize) -> Result<u64, String> {
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
            let key = match mode {
                FsmGroupingMode::Wnbm => {
                    behavior_trace_signature(&raw, NOTEBOOK_BEHAVIOR_TRACE_STEPS)?
                }
                FsmGroupingMode::Moorem => raw_fsm_key(&minimize_raw_fsm(&raw, 0))?,
            };
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

fn decode_fsm_notebook_index_raw(
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

fn canonicalize_raw_fsm(raw: &RawFsm, start_state: usize) -> RawFsm {
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

fn minimize_raw_fsm(raw: &RawFsm, start_state: usize) -> RawFsm {
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

fn raw_fsm_key(raw: &RawFsm) -> Result<Vec<u16>, String> {
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
