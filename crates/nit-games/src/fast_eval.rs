use std::collections::HashMap;

use crate::config::{StrategySpec, StrategySpecKind};
use crate::game::{Action, Outcome, PayoffMatrix};

#[derive(Clone, Debug)]
pub struct FastStrategyModel {
    pub id: String,
    start: u32,
    outputs: Vec<Action>,
    transitions: Vec<u32>,
    alphabet: u32,
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
            StrategySpecKind::Fsm {
                start_state,
                outputs,
                transitions,
                ..
            } => {
                if outputs.is_empty() || transitions.is_empty() {
                    return None;
                }
                let alphabet = transitions.first().map(|row| row.len()).unwrap_or(0);
                if alphabet == 0 {
                    return None;
                }
                let mut flat = Vec::new();
                for row in transitions {
                    if row.len() != alphabet {
                        return None;
                    }
                    for entry in row {
                        flat.push(*entry as u32);
                    }
                }
                Some(Self {
                    id: spec.id.clone(),
                    start: *start_state as u32,
                    outputs: outputs.clone(),
                    transitions: flat,
                    alphabet: alphabet as u32,
                })
            }
            StrategySpecKind::Ca { .. } | StrategySpecKind::OneSidedTm { .. } => None,
        }
    }

    fn action(&self, state: u32) -> Action {
        self.outputs
            .get(state as usize)
            .copied()
            .unwrap_or(Action::Cooperate)
    }

    fn next_state(&self, state: u32, opponent_action: Action) -> u32 {
        let symbol = match opponent_action {
            Action::Cooperate => 0,
            Action::Defect => 1,
        };
        let idx = state.saturating_mul(self.alphabet).saturating_add(symbol);
        self.transitions.get(idx as usize).copied().unwrap_or(state)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct CombinedState {
    a: u32,
    b: u32,
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
        a: a.start,
        b: b.start,
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
        let (a_payoff, b_payoff) = payoff.payoffs(a_action, b_action);
        a_total += a_payoff as i64;
        b_total += b_payoff as i64;
        let outcome = Outcome::from_actions(a_action, b_action);
        counts[outcome.index()] += 1;
        state.a = a.next_state(state.a, b_action);
        state.b = b.next_state(state.b, a_action);
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
