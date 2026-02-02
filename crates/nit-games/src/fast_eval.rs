use std::collections::HashMap;

use crate::config::{BuiltinKind, StrategySpec, StrategySpecKind};
use crate::game::{Action, Outcome, PayoffMatrix};
use crate::strategy::InputMode;

#[derive(Clone, Debug)]
pub struct FastStrategyModel {
    pub id: String,
    kind: FastStrategyKind,
}

#[derive(Clone, Debug)]
enum FastStrategyKind {
    Fsm {
        start: u32,
        outputs: Vec<Action>,
        input_mode: InputMode,
        alphabet: u32,
        transitions: Vec<u32>,
    },
    Memory {
        n: u8,
        initial: Action,
        table: Vec<Action>,
        mask: u64,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum FastState {
    Fsm { state: u32 },
    Memory { filled: u8, window: u64 },
}

#[derive(Clone, Debug)]
pub struct FastEvalResult {
    pub a_total: i64,
    pub b_total: i64,
    pub cycle: Option<CycleMetadata>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CycleMetadata {
    pub transient_rounds: u32,
    pub cycle_rounds: u32,
    pub cycle_cc: u64,
    pub cycle_cd: u64,
    pub cycle_dc: u64,
    pub cycle_dd: u64,
    pub a_cycle_coop_rate: f64,
    pub b_cycle_coop_rate: f64,
}

impl FastStrategyModel {
    pub fn from_spec(spec: &StrategySpec) -> Option<Self> {
        match &spec.kind {
            StrategySpecKind::Builtin { builtin } => Some(Self {
                id: spec.id.clone(),
                kind: builtin_model(*builtin),
            }),
            StrategySpecKind::Random { .. } => None,
            StrategySpecKind::Fsm {
                start_state,
                outputs,
                input_mode,
                transitions,
                ..
            } => {
                let resolved_mode = resolve_fsm_input_mode(*input_mode, transitions);
                let alphabet = resolved_mode.alphabet_size() as u32;
                let mut flat = Vec::new();
                for row in transitions {
                    for entry in row {
                        flat.push(*entry as u32);
                    }
                }
                Some(Self {
                    id: spec.id.clone(),
                    kind: FastStrategyKind::Fsm {
                        start: *start_state as u32,
                        outputs: outputs.clone(),
                        input_mode: resolved_mode,
                        alphabet,
                        transitions: flat,
                    },
                })
            }
            StrategySpecKind::Memory { n, initial, table } => {
                if *n == 0 {
                    return Some(Self {
                        id: spec.id.clone(),
                        kind: FastStrategyKind::Fsm {
                            start: 0,
                            outputs: vec![*initial],
                            input_mode: InputMode::JointLastAction,
                            alphabet: 4,
                            transitions: vec![0, 0, 0, 0],
                        },
                    });
                }
                let n = (*n).min(31) as u8;
                let mask = if n == 0 { 0 } else { (1u64 << (2 * n)) - 1 };
                Some(Self {
                    id: spec.id.clone(),
                    kind: FastStrategyKind::Memory {
                        n,
                        initial: *initial,
                        table: table.clone(),
                        mask,
                    },
                })
            }
            StrategySpecKind::OneSidedTm { .. } => None,
        }
    }

    fn start_state(&self) -> FastState {
        match &self.kind {
            FastStrategyKind::Fsm { start, .. } => FastState::Fsm { state: *start },
            FastStrategyKind::Memory { .. } => FastState::Memory {
                filled: 0,
                window: 0,
            },
        }
    }

    fn action(&self, state: FastState) -> Action {
        match (&self.kind, state) {
            (FastStrategyKind::Fsm { outputs, .. }, FastState::Fsm { state }) => outputs
                .get(state as usize)
                .copied()
                .unwrap_or(Action::Cooperate),
            (
                FastStrategyKind::Memory {
                    n,
                    initial,
                    table,
                    mask,
                },
                FastState::Memory { filled, window },
            ) => {
                if filled < *n {
                    *initial
                } else {
                    let idx = (window & mask) as usize;
                    table.get(idx).copied().unwrap_or(*initial)
                }
            }
            _ => Action::Cooperate,
        }
    }

    fn next_state(&self, state: FastState, outcome: Outcome) -> FastState {
        let idx = outcome.index() as u64;
        match (&self.kind, state) {
            (
                FastStrategyKind::Fsm {
                    transitions,
                    input_mode,
                    alphabet,
                    ..
                },
                FastState::Fsm { state },
            ) => {
                let symbol = input_symbol_from_outcome(*input_mode, outcome) as u32;
                let idx = state.saturating_mul(*alphabet).saturating_add(symbol);
                let next = transitions.get(idx as usize).copied().unwrap_or(state);
                FastState::Fsm { state: next }
            }
            (FastStrategyKind::Memory { n, mask, .. }, FastState::Memory { filled, window }) => {
                if *n == 0 {
                    return FastState::Memory {
                        filled: 0,
                        window: 0,
                    };
                }
                let new_window = ((window << 2) | idx) & mask;
                let new_filled = filled.saturating_add(1).min(*n);
                FastState::Memory {
                    filled: new_filled,
                    window: new_window,
                }
            }
            _ => state,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct CombinedState {
    a: FastState,
    b: FastState,
}

#[derive(Copy, Clone, Debug)]
struct SeenState {
    round: u32,
    a_total: i64,
    b_total: i64,
    counts: [u64; 4],
}

pub fn evaluate_match(
    a: &FastStrategyModel,
    b: &FastStrategyModel,
    rounds: u32,
    payoff: PayoffMatrix,
    record_cycle: bool,
) -> FastEvalResult {
    let mut state = CombinedState {
        a: a.start_state(),
        b: b.start_state(),
    };
    let mut seen: HashMap<CombinedState, SeenState> =
        HashMap::with_capacity(rounds.min(4096) as usize);
    let mut round: u32 = 0;
    let mut a_total: i64 = 0;
    let mut b_total: i64 = 0;
    let mut counts: [u64; 4] = [0; 4];
    let mut cycle_meta: Option<CycleMetadata> = None;
    let mut detect_cycles = true;

    while round < rounds {
        if detect_cycles {
            if let Some(prev) = seen.get(&state) {
                let cycle_len = round.saturating_sub(prev.round);
                if cycle_len > 0 {
                    let cycle_counts = [
                        counts[0].saturating_sub(prev.counts[0]),
                        counts[1].saturating_sub(prev.counts[1]),
                        counts[2].saturating_sub(prev.counts[2]),
                        counts[3].saturating_sub(prev.counts[3]),
                    ];
                    if record_cycle && cycle_meta.is_none() {
                        cycle_meta =
                            Some(build_cycle_metadata(prev.round, cycle_len, cycle_counts));
                    }
                    let cycle_a_total = a_total.saturating_sub(prev.a_total);
                    let cycle_b_total = b_total.saturating_sub(prev.b_total);
                    let remaining = rounds - round;
                    let cycles = remaining / cycle_len;
                    if cycles > 0 {
                        a_total += cycle_a_total * cycles as i64;
                        b_total += cycle_b_total * cycles as i64;
                        for i in 0..4 {
                            counts[i] += cycle_counts[i] * cycles as u64;
                        }
                        round += cycle_len * cycles;
                    }
                }
                detect_cycles = false;
                if round >= rounds {
                    break;
                }
            } else {
                seen.insert(
                    state,
                    SeenState {
                        round,
                        a_total,
                        b_total,
                        counts,
                    },
                );
            }
        }

        if round >= rounds {
            break;
        }

        let a_action = a.action(state.a);
        let b_action = b.action(state.b);
        let outcome_a = Outcome::from_actions(a_action, b_action);
        let outcome_b = Outcome::from_actions(b_action, a_action);
        let (a_payoff, b_payoff) = payoff.payoffs(a_action, b_action);
        a_total += a_payoff as i64;
        b_total += b_payoff as i64;
        counts[outcome_a.index()] += 1;
        state.a = a.next_state(state.a, outcome_a);
        state.b = b.next_state(state.b, outcome_b);
        round += 1;
    }

    FastEvalResult {
        a_total,
        b_total,
        cycle: cycle_meta,
    }
}

fn build_cycle_metadata(
    transient_rounds: u32,
    cycle_rounds: u32,
    counts: [u64; 4],
) -> CycleMetadata {
    let [cc, cd, dc, dd] = counts;
    let total = cycle_rounds.max(1) as f64;
    let a_coop = (cc + cd) as f64 / total;
    let b_coop = (cc + dc) as f64 / total;
    CycleMetadata {
        transient_rounds,
        cycle_rounds,
        cycle_cc: cc,
        cycle_cd: cd,
        cycle_dc: dc,
        cycle_dd: dd,
        a_cycle_coop_rate: a_coop,
        b_cycle_coop_rate: b_coop,
    }
}

fn resolve_fsm_input_mode(input_mode: Option<InputMode>, transitions: &[Vec<usize>]) -> InputMode {
    if let Some(mode) = input_mode {
        return mode;
    }
    let first_len = transitions.first().map(|row| row.len()).unwrap_or(2);
    if first_len == 4 {
        InputMode::JointLastAction
    } else {
        InputMode::OpponentLastAction
    }
}

fn input_symbol_from_outcome(mode: InputMode, outcome: Outcome) -> usize {
    match mode {
        InputMode::OpponentLastAction => match outcome {
            Outcome::CC | Outcome::DC => 0,
            Outcome::CD | Outcome::DD => 1,
        },
        InputMode::SelfLastAction => match outcome {
            Outcome::CC | Outcome::CD => 0,
            Outcome::DC | Outcome::DD => 1,
        },
        InputMode::JointLastAction => outcome.index(),
    }
}

fn builtin_model(builtin: BuiltinKind) -> FastStrategyKind {
    match builtin {
        BuiltinKind::AllC => FastStrategyKind::Fsm {
            start: 0,
            outputs: vec![Action::Cooperate],
            input_mode: InputMode::JointLastAction,
            alphabet: 4,
            transitions: vec![0, 0, 0, 0],
        },
        BuiltinKind::AllD => FastStrategyKind::Fsm {
            start: 0,
            outputs: vec![Action::Defect],
            input_mode: InputMode::JointLastAction,
            alphabet: 4,
            transitions: vec![0, 0, 0, 0],
        },
        BuiltinKind::TitForTat => FastStrategyKind::Fsm {
            start: 0,
            outputs: vec![Action::Cooperate, Action::Defect],
            input_mode: InputMode::JointLastAction,
            alphabet: 4,
            transitions: vec![0, 1, 0, 1, 0, 1, 0, 1],
        },
        BuiltinKind::GrimTrigger => FastStrategyKind::Fsm {
            start: 0,
            outputs: vec![Action::Cooperate, Action::Defect],
            input_mode: InputMode::JointLastAction,
            alphabet: 4,
            transitions: vec![0, 1, 0, 1, 1, 1, 1, 1],
        },
        BuiltinKind::WinStayLoseShift => FastStrategyKind::Fsm {
            start: 0,
            outputs: vec![Action::Cooperate, Action::Defect],
            input_mode: InputMode::JointLastAction,
            alphabet: 4,
            transitions: vec![0, 1, 1, 0, 1, 0, 0, 1],
        },
    }
}
