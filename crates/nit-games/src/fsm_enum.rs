use crate::config::{StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::InputMode;
use nit_utils::hashing::stable_hash_bytes;
use std::collections::VecDeque;

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
    let mut map = vec![None; def.num_states];
    let mut order = Vec::new();
    let mut queue = VecDeque::new();
    map[def.start_state] = Some(0);
    queue.push_back(def.start_state);

    while let Some(state) = queue.pop_front() {
        order.push(state);
        if let Some(row) = def.transitions.get(state) {
            for symbol in 0..alphabet {
                let next = row.get(symbol).copied().unwrap_or(state);
                if next < def.num_states && map[next].is_none() {
                    map[next] = Some(order.len());
                    queue.push_back(next);
                }
            }
        }
    }

    let reachable = order.len();
    let mut outputs = Vec::with_capacity(reachable);
    for &state in &order {
        outputs.push(def.outputs[state]);
    }

    let mut transitions = Vec::with_capacity(reachable);
    for &state in &order {
        let mut row = Vec::with_capacity(alphabet);
        for symbol in 0..alphabet {
            let next = def.transitions[state][symbol];
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

            let output_spec = if canonical {
                canonicalize_fsm(&spec)
            } else {
                spec
            };
            let key = output_spec.stable_key();
            if !canonical || seen.insert(key) {
                emit(output_spec);
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
