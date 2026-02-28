use crate::config::{FsmGroupingMode, StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::InputMode;
use nit_utils::hashing::stable_hash_bytes;
use std::collections::{HashMap, VecDeque};

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

pub fn canonicalize_fsm(def: &FsmDefinition) -> FsmDefinition {
    let alphabet = def.input_mode.alphabet_size();
    if def.num_states == 0 || def.outputs.is_empty() {
        return def.clone();
    }
    let start_state = def.start_state.min(def.num_states.saturating_sub(1));
    let mut map = vec![None; def.num_states];
    let mut order = Vec::new();
    let mut queue = VecDeque::new();
    let mut next_id = 1usize;
    map[start_state] = Some(0);
    queue.push_back(start_state);

    while let Some(state) = queue.pop_front() {
        order.push(state);
        if let Some(row) = def.transitions.get(state) {
            for symbol in 0..alphabet {
                let next = row
                    .get(symbol)
                    .copied()
                    .filter(|candidate| *candidate < def.num_states)
                    .unwrap_or(state);
                if next < def.num_states && map[next].is_none() {
                    map[next] = Some(next_id);
                    next_id += 1;
                    queue.push_back(next);
                }
            }
        }
    }

    let reachable = order.len();
    let mut outputs = Vec::with_capacity(reachable);
    for &state in &order {
        outputs.push(def.outputs.get(state).copied().unwrap_or(Action::Cooperate));
    }

    let mut transitions = Vec::with_capacity(reachable);
    for &state in &order {
        let mut row = Vec::with_capacity(alphabet);
        for symbol in 0..alphabet {
            let next = def
                .transitions
                .get(state)
                .and_then(|row| row.get(symbol))
                .copied()
                .filter(|candidate| *candidate < def.num_states)
                .unwrap_or(state);
            let mapped = map[next].unwrap_or(0);
            row.push(mapped);
        }
        transitions.push(row);
    }

    FsmDefinition {
        num_states: reachable,
        start_state: 0,
        outputs,
        input_mode: def.input_mode,
        transitions,
    }
}

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
                if seen.insert(key) {
                    emit(output_spec);
                    count += 1;
                    if let Some(limit) = limit {
                        if count >= limit {
                            return count;
                        }
                    }
                }
            } else {
                emit(spec);
                count += 1;
                if let Some(limit) = limit {
                    if count >= limit {
                        return count;
                    }
                }
            }

            let mut idx = 0usize;
            loop {
                if idx >= num_states * alphabet {
                    done = true;
                    break;
                }
                let row = idx / alphabet;
                let col = idx % alphabet;
                if transitions[row][col] + 1 < num_states {
                    transitions[row][col] += 1;
                    break;
                } else {
                    transitions[row][col] = 0;
                    idx += 1;
                }
            }
        }
    }

    count
}

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

pub fn canonical_fsm_indices(states: usize, actions: usize) -> Result<Vec<u64>, String> {
    let all = fsm_indices(states, actions)?;
    let mut first_by_key: HashMap<Vec<u16>, u64> = HashMap::new();
    for idx in all {
        let raw = decode_fsm_notebook_index_raw(idx, states, actions)?;
        let canonical = canonicalize_raw_fsm(&raw, 0);
        let key = raw_fsm_key(&canonical)?;
        first_by_key.entry(key).or_insert(idx);
    }
    let mut canonical: Vec<u64> = first_by_key.into_values().collect();
    canonical.sort_unstable();
    Ok(canonical)
}

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
    let groups = group_canonical_fsm_indices_by_behavior_with_mode(states, actions, mode)?;
    Ok(groups
        .into_iter()
        .filter_map(|group| group.into_iter().next())
        .collect())
}

fn fsm_indices(states: usize, actions: usize) -> Result<Vec<u64>, String> {
    let Some(count) = crate::strategy::fsm_count(states, actions) else {
        return Err("fsm index space overflows u128 for this (states, actions)".to_string());
    };
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > u64::MAX as u128 {
        return Err("fsm index space exceeds u64 range".to_string());
    }
    Ok((0..(count as u64)).collect())
}

fn decode_fsm_notebook_index_raw(
    index: u64,
    states: usize,
    actions: usize,
) -> Result<RawFsm, String> {
    if states == 0 {
        return Err("fsm decode requires states > 0".to_string());
    }
    if actions == 0 {
        return Err("fsm decode requires actions > 0".to_string());
    }
    let Some(max) = crate::strategy::fsm_count(states, actions) else {
        return Err("fsm index space overflows u128 for this (states, actions)".to_string());
    };
    if index as u128 >= max {
        return Err(format!("fsm index {index} out of range (0..{})", max - 1));
    }

    let transition_digits = states.saturating_mul(actions);
    let Some(action_block) = checked_pow_u128(actions as u128, states as u32) else {
        return Err("fsm action block overflows u128".to_string());
    };
    let (transition_code, output_code) =
        floor_div_rem_i128(index as i128 - 1, action_block as i128);

    let transitions_flat = if states == 1 {
        vec![0usize; transition_digits]
    } else {
        integer_digits_unsigned(transition_code.unsigned_abs(), states, transition_digits)
    };
    let outputs = if actions == 1 {
        vec![0usize; states]
    } else {
        integer_digits_unsigned(output_code as u128, actions, states)
    };

    let mut transitions = vec![vec![0usize; actions]; states];
    for state_idx in 0..states {
        for input_idx in 0..actions {
            let flat_idx = state_idx.saturating_mul(actions).saturating_add(input_idx);
            let next = transitions_flat.get(flat_idx).copied().unwrap_or(0);
            transitions[state_idx][input_idx] = next.min(states - 1);
        }
    }

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
    for state in 0..state_count {
        let output = machine.outputs[state];
        let block = if let Some(existing) = output_blocks.get(&output).copied() {
            existing
        } else {
            let next_block = output_blocks.len();
            output_blocks.insert(output, next_block);
            next_block
        };
        block_by_state[state] = block;
    }

    loop {
        let mut signature_blocks: HashMap<Vec<usize>, usize> = HashMap::new();
        let mut refined = vec![0usize; state_count];
        for state in 0..state_count {
            let mut signature = Vec::with_capacity(machine.actions + 1);
            signature.push(machine.outputs[state]);
            for input in 0..machine.actions {
                let next = machine.transitions[state][input];
                signature.push(block_by_state[next]);
            }
            let block = if let Some(existing) = signature_blocks.get(&signature).copied() {
                existing
            } else {
                let next_block = signature_blocks.len();
                signature_blocks.insert(signature, next_block);
                next_block
            };
            refined[state] = block;
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
        for input in 0..machine.actions {
            let next = machine.transitions[state][input];
            transitions[block][input] = block_by_state[next];
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

fn floor_div_rem_i128(numer: i128, denom: i128) -> (i128, i128) {
    let mut q = numer / denom;
    let mut r = numer % denom;
    if r < 0 {
        q -= 1;
        r += denom;
    }
    (q, r)
}

fn integer_digits_unsigned(mut value: u128, base: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let base_u128 = base.max(2) as u128;
    let mut digits = vec![0usize; len];
    for idx in (0..len).rev() {
        digits[idx] = (value % base_u128) as usize;
        value /= base_u128;
    }
    digits
}

fn checked_pow_u128(base: u128, exp: u32) -> Option<u128> {
    let mut value = 1u128;
    for _ in 0..exp {
        value = value.checked_mul(base)?;
    }
    Some(value)
}
