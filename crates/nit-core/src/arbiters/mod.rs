//! Arbiter framework — fourth role in the living-system taxonomy.
//!
//! Arbiters read the substrate at tick boundaries (TurnCompleted, metabolic
//! tick), detect structural failures (persistent conflicts, deadlocks, stuck
//! slots), and produce interventions that actuate via nit-tui's existing
//! retry/dispatch infrastructure.
//!
//! Invariant: arbiters run AFTER observers in every tick. They see the same
//! `AppState` snapshot observers just read, plus any signals observers
//! emitted. No arbiter reads `InterventionEmitted` — prevents self-loops.

use std::collections::{HashMap, HashSet};

use crate::state::{AppState, Intervention};
use crate::substrate::{Signal, SignalKind, SignalTarget};

pub mod help_needed;
pub mod persistent_conflict;
pub mod sparse_plan_arbiter;

pub const ARBITER_INITIAL_STRENGTH: f32 = 2.0;
pub const ARBITER_COOLDOWN_GENS: u64 = 10;

/// Default; mood modulation overrides via `mood.modulation().arbiter_max_per_tick`.
pub const ARBITER_MAX_PER_TICK: usize = 2;

/// Mirror of nit-tui's `GENOME_RETRY_LIMIT`. Equality is enforced by the
/// `genome_retry_limit_matches_arbiter_retry_limit` test in nit-tui.
pub const ARBITER_RETRY_LIMIT: u8 = 3;

#[derive(Clone, Debug)]
pub struct InterventionProposal {
    pub kind: InterventionKind,
    pub target: InterventionTarget,
    pub rationale: String,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug)]
pub enum InterventionKind {
    RedispatchWithEscalatedPrompt { prompt: String },
    EmitSignalOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InterventionTarget {
    Agent { agent_id: String },
    AgentPair { a: String, b: String },
    Mission { mission_id: String },
    Global,
}

impl InterventionTarget {
    /// Map this target to a `SignalTarget` for emission and cooldown matching.
    pub fn signal_target(&self) -> SignalTarget {
        match self {
            InterventionTarget::Agent { agent_id }
            | InterventionTarget::AgentPair { a: agent_id, .. } => SignalTarget::Agent {
                agent_id: agent_id.clone(),
            },
            InterventionTarget::Mission { .. } | InterventionTarget::Global => SignalTarget::Global,
        }
    }

    /// Whether a previously-emitted signal targets the same scope as this proposal.
    pub fn matches_signal(&self, signal_target: &SignalTarget) -> bool {
        match (self, signal_target) {
            (
                InterventionTarget::Agent { agent_id: own },
                SignalTarget::Agent { agent_id: other },
            ) => own == other,
            (InterventionTarget::AgentPair { a, b }, SignalTarget::Agent { agent_id: other }) => {
                a == other || b == other
            }
            (InterventionTarget::Global, SignalTarget::Global)
            | (InterventionTarget::Mission { .. }, SignalTarget::Global) => true,
            _ => false,
        }
    }

    /// Per-agent lookup so a burnt-out agent does not silence interventions
    /// targeting other agents running in parallel.
    pub fn budget_exhausted(&self, retries: &HashMap<String, u8>, limit: u8) -> bool {
        match self {
            InterventionTarget::Agent { agent_id } => {
                retries.get(agent_id).copied().unwrap_or(0) >= limit
            }
            InterventionTarget::AgentPair { a, b } => {
                let count_a = retries.get(a).copied().unwrap_or(0);
                let count_b = retries.get(b).copied().unwrap_or(0);
                count_a.min(count_b) >= limit
            }
            InterventionTarget::Mission { .. } | InterventionTarget::Global => {
                retries.values().copied().max().unwrap_or(0) >= limit
            }
        }
    }

    /// Serializable scope summary for the InterventionEmitted signal payload.
    pub fn payload_object(&self) -> serde_json::Value {
        match self {
            InterventionTarget::Agent { agent_id } => serde_json::json!({ "agent": agent_id }),
            InterventionTarget::AgentPair { a, b } => serde_json::json!({ "pair": [a, b] }),
            InterventionTarget::Mission { mission_id } => {
                serde_json::json!({ "mission": mission_id })
            }
            InterventionTarget::Global => serde_json::json!({ "scope": "global" }),
        }
    }
}

pub type ArbiterFn = fn(&AppState) -> Vec<InterventionProposal>;

pub struct Arbiter {
    pub name: &'static str,
    pub run: ArbiterFn,
}

pub const REGISTERED_ARBITERS: &[Arbiter] = &[
    persistent_conflict::ARBITER,
    sparse_plan_arbiter::ARBITER,
    help_needed::ARBITER,
];

pub fn run_all(state: &AppState) -> Vec<(&'static str, InterventionProposal)> {
    let mut all_proposals = Vec::new();
    for arbiter in REGISTERED_ARBITERS {
        for proposal in (arbiter.run)(state) {
            all_proposals.push((arbiter.name, proposal));
        }
    }
    all_proposals
}

/// Apply per-(arbiter, target) cooldown, per-tick budget, and downgrade to
/// `EmitSignalOnly` when the retry budget is exhausted. Does NOT consume the
/// retry budget — actuation (dispatch) does that.
pub fn reduce_proposals(
    state: &AppState,
    raw: Vec<(&'static str, InterventionProposal)>,
    genome_retry_limit: u8,
) -> Vec<Intervention> {
    let current_gen = state.substrate.current_generation();
    let cooldown_start = current_gen.saturating_sub(ARBITER_COOLDOWN_GENS);
    let max_per_tick = state.substrate.mood.modulation().arbiter_max_per_tick;
    let retry_counts = &state.genome_retry_counts;

    let mut accepted: Vec<Intervention> = Vec::new();
    for (arbiter_name, proposal) in raw {
        if in_cooldown(state, arbiter_name, &proposal.target, cooldown_start) {
            continue;
        }
        let outcome_kind = if proposal
            .target
            .budget_exhausted(retry_counts, genome_retry_limit)
        {
            InterventionKind::EmitSignalOnly
        } else {
            proposal.kind.clone()
        };
        accepted.push(Intervention {
            arbiter_name,
            kind: outcome_kind,
            target: proposal.target,
            rationale: proposal.rationale,
            payload: proposal.payload,
            decided_at_gen: current_gen,
        });
        if accepted.len() >= max_per_tick {
            break;
        }
    }
    accepted
}

fn in_cooldown(
    state: &AppState,
    arbiter_name: &str,
    target: &InterventionTarget,
    cooldown_start: u64,
) -> bool {
    let posted_by = format!("arbiter:{arbiter_name}");
    state.substrate.signals.values().any(|signal| {
        signal.kind == SignalKind::InterventionEmitted
            && signal.posted_by == posted_by
            && signal.posted_at_gen >= cooldown_start
            && target.matches_signal(&signal.target)
    })
}

/// Emit the InterventionEmitted signal for each reduced intervention and push
/// it onto the queue for nit-tui to drain.
pub fn apply_interventions(state: &mut AppState, interventions: Vec<Intervention>) {
    for intervention in interventions {
        let signal_target = intervention.target.signal_target();
        let posted_by = format!("arbiter:{}", intervention.arbiter_name);
        let id = state.substrate.next_signal_id(&posted_by);
        let posted_at_gen = state.substrate.current_generation();
        state.substrate.emit_signal(crate::substrate::Signal {
            id,
            kind: SignalKind::InterventionEmitted,
            posted_by,
            posted_at_gen,
            target: signal_target,
            initial_strength: ARBITER_INITIAL_STRENGTH,
            payload: serde_json::json!({
                "rationale": intervention.rationale,
                "target": intervention.target.payload_object(),
                "details": intervention.payload.clone(),
            }),
        });
        state.pending_interventions.push(intervention);
    }
}

/// Scan recent `HelpNeeded` signals from a specific observer, deduped per
/// agent, and let the caller build one `InterventionProposal` per agent.
///
/// Encapsulates the gen-window filter, kind/poster guard, agent-target
/// extraction, and per-agent dedup that `help_needed` and `sparse_plan_arbiter`
/// both need.
pub(super) fn scan_help_needed_signals<F>(
    state: &AppState,
    observer_name: &str,
    window_gens: u64,
    mut build: F,
) -> Vec<InterventionProposal>
where
    F: FnMut(&Signal, &str) -> InterventionProposal,
{
    let substrate = &state.substrate;
    let window_start = substrate.current_generation().saturating_sub(window_gens);

    let mut proposals = Vec::new();
    let mut seen_agents: HashSet<String> = HashSet::new();
    for signal in substrate.signals.values() {
        if signal.posted_at_gen < window_start {
            continue;
        }
        if signal.kind != SignalKind::HelpNeeded || signal.posted_by != observer_name {
            continue;
        }
        let SignalTarget::Agent { agent_id } = &signal.target else {
            continue;
        };
        if !seen_agents.insert(agent_id.clone()) {
            continue;
        }
        proposals.push(build(signal, agent_id));
    }
    proposals
}

#[cfg(test)]
#[path = "../tests/arbiters.rs"]
mod tests;
