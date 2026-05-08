//! Fast cycle-detecting match evaluation for FSM strategies.
//!
//! Transitions are flattened into a single `Vec<u32>` indexed by
//! `state * alphabet + symbol` so the inner loop walks contiguous
//! memory and stays in L1.

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
    /// Per-round outcome digit string (`'0'`=CC, `'1'`=CD, `'2'`=DC, `'3'`=DD).
    pub outcomes: Option<String>,
}

/// Two deterministic FSMs always cycle eventually; this captures the
/// transient prefix length, cycle length, and per-outcome distribution
/// across one period so the remaining rounds can be fast-forwarded.
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
    /// Returns `None` for non-FSM strategies, empty/jagged transition
    /// tables, or any FSM whose transition rows do not all share the
    /// same alphabet width.
    pub fn from_spec(spec: &StrategySpec) -> Option<Self> {
        let StrategySpecKind::Fsm {
            start_state,
            outputs,
            transitions,
            ..
        } = &spec.kind
        else {
            return None;
        };
        if outputs.is_empty() || transitions.is_empty() {
            return None;
        }
        let alphabet = transitions.first().map(|row| row.len()).unwrap_or(0);
        if alphabet == 0 {
            return None;
        }
        let mut flat = Vec::with_capacity(transitions.len() * alphabet);
        for row in transitions {
            if row.len() != alphabet {
                return None;
            }
            flat.extend(row.iter().map(|&val| val as u32));
        }
        Some(Self {
            id: spec.id.clone(),
            start: *start_state as u32,
            outputs: outputs.clone(),
            transitions: flat,
            alphabet: alphabet as u32,
        })
    }

    fn action(&self, state: u32) -> Action {
        self.outputs
            .get(state as usize)
            .copied()
            .unwrap_or(Action::Cooperate)
    }

    fn next_state(&self, current: u32, opponent_action: Action) -> u32 {
        let symbol = match opponent_action {
            Action::Cooperate => 0,
            Action::Defect => 1,
        };
        let idx = current.saturating_mul(self.alphabet).saturating_add(symbol);
        self.transitions
            .get(idx as usize)
            .copied()
            .unwrap_or(current)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct JointState {
    a: u32,
    b: u32,
}

#[derive(Copy, Clone, Debug)]
struct Snapshot {
    round: u32,
    a_total: i64,
    b_total: i64,
    outcome_counts: [u64; 4],
}

/// Plays `rounds` rounds between two flattened FSMs. The combined
/// `(state_a, state_b)` is hashed each round; once a pair repeats we
/// know we are in a cycle and replicate the deltas across the
/// remaining rounds in O(1).
pub fn evaluate_match(
    model_a: &FastStrategyModel,
    model_b: &FastStrategyModel,
    rounds: u32,
    payoff: PayoffMatrix,
    record_cycle: bool,
    record_outcomes: bool,
) -> FastEvalResult {
    let capacity = rounds.min(4096) as usize;
    let mut joint = JointState {
        a: model_a.start,
        b: model_b.start,
    };
    let mut seen: HashMap<JointState, Snapshot> = HashMap::with_capacity(capacity);
    let mut round: u32 = 0;
    let mut a_total: i64 = 0;
    let mut b_total: i64 = 0;
    let mut outcome_counts: [u64; 4] = [0; 4];
    let mut cycle_meta: Option<CycleMetadata> = None;
    let mut detect_cycles = true;
    let mut outcomes = record_outcomes.then(|| Vec::with_capacity(capacity));

    while round < rounds {
        if detect_cycles {
            if let Some(snap) = seen.get(&joint).copied() {
                if let Some(advanced) = fast_forward_cycle(
                    snap,
                    round,
                    rounds,
                    &mut a_total,
                    &mut b_total,
                    &mut outcome_counts,
                    outcomes.as_mut(),
                    record_cycle,
                    &mut cycle_meta,
                ) {
                    round = advanced;
                }
                detect_cycles = false;
                if round >= rounds {
                    break;
                }
            } else {
                seen.insert(
                    joint,
                    Snapshot {
                        round,
                        a_total,
                        b_total,
                        outcome_counts,
                    },
                );
            }
        }

        let act_a = model_a.action(joint.a);
        let act_b = model_b.action(joint.b);
        let (pay_a, pay_b) = payoff.payoffs(act_a, act_b);
        a_total += pay_a as i64;
        b_total += pay_b as i64;
        let outcome = Outcome::from_actions(act_a, act_b);
        outcome_counts[outcome.index()] += 1;
        if let Some(buf) = outcomes.as_mut() {
            buf.push(outcome.digit_byte());
        }
        joint.a = model_a.next_state(joint.a, act_b);
        joint.b = model_b.next_state(joint.b, act_a);
        round += 1;
    }

    FastEvalResult {
        a_total,
        b_total,
        cycle: cycle_meta,
        outcomes: outcomes.and_then(|bytes| String::from_utf8(bytes).ok()),
    }
}

#[allow(clippy::too_many_arguments)]
fn fast_forward_cycle(
    snap: Snapshot,
    round: u32,
    rounds: u32,
    a_total: &mut i64,
    b_total: &mut i64,
    outcome_counts: &mut [u64; 4],
    outcomes: Option<&mut Vec<u8>>,
    record_cycle: bool,
    cycle_meta: &mut Option<CycleMetadata>,
) -> Option<u32> {
    let period = round.saturating_sub(snap.round);
    if period == 0 {
        return None;
    }
    let delta_counts = [
        outcome_counts[0].saturating_sub(snap.outcome_counts[0]),
        outcome_counts[1].saturating_sub(snap.outcome_counts[1]),
        outcome_counts[2].saturating_sub(snap.outcome_counts[2]),
        outcome_counts[3].saturating_sub(snap.outcome_counts[3]),
    ];
    if record_cycle && cycle_meta.is_none() {
        let [cc, cd, dc, dd] = delta_counts;
        let denom = period.max(1) as f64;
        *cycle_meta = Some(CycleMetadata {
            transient_rounds: snap.round,
            cycle_rounds: period,
            cycle_cc: cc,
            cycle_cd: cd,
            cycle_dc: dc,
            cycle_dd: dd,
            a_cycle_coop_rate: (cc + cd) as f64 / denom,
            b_cycle_coop_rate: (cc + dc) as f64 / denom,
        });
    }
    let delta_a = a_total.saturating_sub(snap.a_total);
    let delta_b = b_total.saturating_sub(snap.b_total);
    let full_cycles = (rounds - round) / period;
    *a_total += delta_a * full_cycles as i64;
    *b_total += delta_b * full_cycles as i64;
    for (acc, delta) in outcome_counts.iter_mut().zip(delta_counts.iter()) {
        *acc += delta * full_cycles as u64;
    }
    replicate_cycle_outcomes(outcomes, snap.round, round, full_cycles);
    Some(round + period * full_cycles)
}

fn replicate_cycle_outcomes(
    buf: Option<&mut Vec<u8>>,
    cycle_start: u32,
    cycle_end: u32,
    full_cycles: u32,
) {
    let Some(history) = buf else { return };
    if full_cycles == 0 {
        return;
    }
    let range = cycle_start as usize..cycle_end as usize;
    let cycle_len = range.len();
    history.reserve(cycle_len.saturating_mul(full_cycles as usize));
    for _ in 0..full_cycles {
        history.extend_from_within(range.clone());
    }
}
