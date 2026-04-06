//! Fast match evaluation for FSM strategies.
//!
//! This module provides [`FastStrategyModel`], a flattened representation of an
//! FSM strategy optimised for cache-friendly match simulation, and
//! [`evaluate_match`], which runs a full match between two FSM strategies with
//! optional cycle detection and outcome recording.

use std::collections::HashMap;

use crate::config::{StrategySpec, StrategySpecKind};
use crate::game::{Action, Outcome, PayoffMatrix};

/// A flattened, evaluation-optimised representation of an FSM strategy.
///
/// Transition rows are stored in a contiguous `Vec<u32>` indexed by
/// `state * alphabet + symbol`, which avoids per-row `Vec` overhead and
/// improves cache locality during match simulation.
#[derive(Clone, Debug)]
pub struct FastStrategyModel {
    pub id: String,
    start: u32,
    outputs: Vec<Action>,
    transitions: Vec<u32>,
    alphabet: u32,
}

/// The result of evaluating a single match between two FSM strategies.
#[derive(Clone, Debug)]
pub struct FastEvalResult {
    /// Cumulative payoff for strategy A over all rounds.
    pub a_total: i64,
    /// Cumulative payoff for strategy B over all rounds.
    pub b_total: i64,
    /// Cycle metadata, populated when cycle detection is requested and a cycle
    /// was found.
    pub cycle: Option<CycleMetadata>,
    /// Encoded outcome string (`'0'`=CC, `'1'`=CD, `'2'`=DC, `'3'`=DD), present
    /// only when outcome recording is requested.
    pub outcomes: Option<String>,
}

/// Metadata describing the periodic cycle discovered during match evaluation.
///
/// A match between two deterministic FSM strategies always eventually enters a
/// cycle. This struct captures where the cycle starts, how long it is, and the
/// outcome distribution within the cycle.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CycleMetadata {
    /// Number of rounds before the cycle begins (the transient prefix).
    pub transient_rounds: u32,
    /// Length of the repeating cycle in rounds.
    pub cycle_rounds: u32,
    /// Count of CC (mutual cooperation) outcomes within one cycle.
    pub cycle_cc: u64,
    /// Count of CD (A cooperates, B defects) outcomes within one cycle.
    pub cycle_cd: u64,
    /// Count of DC (A defects, B cooperates) outcomes within one cycle.
    pub cycle_dc: u64,
    /// Count of DD (mutual defection) outcomes within one cycle.
    pub cycle_dd: u64,
    /// Fraction of cycle rounds where strategy A cooperates.
    pub a_cycle_coop_rate: f64,
    /// Fraction of cycle rounds where strategy B cooperates.
    pub b_cycle_coop_rate: f64,
}

impl FastStrategyModel {
    /// Try to build a [`FastStrategyModel`] from a [`StrategySpec`].
    ///
    /// Returns `None` for non-FSM strategies or if the FSM data is malformed
    /// (empty outputs, empty transitions, or inconsistent row widths).
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

    /// Return the action (output) for the given FSM state.
    fn action(&self, state: u32) -> Action {
        self.outputs
            .get(state as usize)
            .copied()
            .unwrap_or(Action::Cooperate)
    }

    /// Compute the next FSM state given the current state and the opponent's
    /// last action.
    fn next_state(&self, state: u32, opponent_action: Action) -> u32 {
        let symbol = match opponent_action {
            Action::Cooperate => 0,
            Action::Defect => 1,
        };
        let idx = state.saturating_mul(self.alphabet).saturating_add(symbol);
        self.transitions.get(idx as usize).copied().unwrap_or(state)
    }
}

/// The joint state of two FSM strategies at a given point during a match.
/// Field `a` is the current FSM state of player A; `b` is player B's.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct CombinedState {
    a: u32,
    b: u32,
}

/// A snapshot of the match at the first visit to a particular [`CombinedState`],
/// used for cycle detection.
#[derive(Copy, Clone, Debug)]
struct SeenState {
    round: u32,
    a_total: i64,
    b_total: i64,
    counts: [u64; 4],
}

/// Evaluate a single match between two FSM strategies over a fixed number of
/// rounds, optionally recording cycle metadata and per-round outcomes.
///
/// Uses cycle detection: when the combined state revisits a previously-seen
/// pair, the remaining rounds are fast-forwarded using the per-cycle payoff
/// totals.
pub fn evaluate_match(
    a: &FastStrategyModel,
    b: &FastStrategyModel,
    rounds: u32,
    payoff: PayoffMatrix,
    record_cycle: bool,
    record_outcomes: bool,
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
    let mut outcomes = record_outcomes.then(|| Vec::with_capacity(rounds.min(4096) as usize));

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
                        if let Some(history) = outcomes.as_mut() {
                            let cycle = history[prev.round as usize..round as usize].to_vec();
                            history.reserve(cycle.len().saturating_mul(cycles as usize));
                            for _ in 0..cycles {
                                history.extend_from_slice(&cycle);
                            }
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
        if let Some(history) = outcomes.as_mut() {
            history.push(match outcome {
                Outcome::CC => b'0',
                Outcome::CD => b'1',
                Outcome::DC => b'2',
                Outcome::DD => b'3',
            });
        }
        state.a = a.next_state(state.a, b_action);
        state.b = b.next_state(state.b, a_action);
        round += 1;
    }

    FastEvalResult {
        a_total,
        b_total,
        cycle: cycle_meta,
        outcomes: outcomes.and_then(|bytes| String::from_utf8(bytes).ok()),
    }
}

/// Build a [`CycleMetadata`] from the transient prefix length, cycle length,
/// and the four outcome counts within a single cycle period.
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

#[cfg(test)]
#[path = "test_modules/fast_eval.rs"]
mod tests;
