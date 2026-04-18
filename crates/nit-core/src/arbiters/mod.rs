//! Arbiter framework — the fourth role in the living-system taxonomy.
//!
//! Arbiters read the substrate at tick boundaries (TurnCompleted,
//! metabolic tick), detect structural failures (persistent conflicts,
//! deadlocks, stuck slots), and produce interventions that actuate via
//! nit-tui's existing retry/dispatch infrastructure.
//!
//! Invariant: arbiters run AFTER observers in every tick. They see the
//! same AppState snapshot observers just read, plus any signals observers
//! emitted. No arbiter reads `InterventionEmitted` — prevents self-loops.

use crate::state::{AppState, Intervention};
use crate::substrate::{SignalKind, SignalTarget};

pub mod persistent_conflict;
pub mod sparse_plan_arbiter;

pub const OBSERVER_INITIAL_STRENGTH: f32 = 1.5; // reference — same as observers
pub const ARBITER_INITIAL_STRENGTH: f32 = 2.0;
pub const ARBITER_COOLDOWN_GENS: u64 = 10;
pub const ARBITER_MAX_PER_TICK: usize = 2;

/// Mirror of nit-tui's `GENOME_RETRY_LIMIT` (3). Kept here so
/// `agent_bus` and `metabolism` can pass a retry cap into
/// `reduce_proposals` without depending on nit-tui. Must stay in sync
/// with `crates/nit-tui/src/app/mod.rs::GENOME_RETRY_LIMIT`.
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

pub type ArbiterFn = fn(&AppState) -> Vec<InterventionProposal>;

pub struct Arbiter {
    pub name: &'static str,
    pub run: ArbiterFn,
}

pub const REGISTERED_ARBITERS: &[Arbiter] =
    &[persistent_conflict::ARBITER, sparse_plan_arbiter::ARBITER];

pub fn run_all(state: &AppState) -> Vec<(&'static str, InterventionProposal)> {
    let mut out = Vec::new();
    for arb in REGISTERED_ARBITERS {
        for p in (arb.run)(state) {
            out.push((arb.name, p));
        }
    }
    out
}

/// Apply safety guards: per-(arbiter, target) cooldown; per-tick budget;
/// downgrade to EmitSignalOnly if the retry budget is exhausted.
///
/// Does NOT consume budget — actuation (dispatch) does that.
pub fn reduce_proposals(
    state: &AppState,
    raw: Vec<(&'static str, InterventionProposal)>,
    genome_retry_limit: u8,
) -> Vec<Intervention> {
    let current_gen = state.substrate.current_generation();
    let cooldown_start = current_gen.saturating_sub(ARBITER_COOLDOWN_GENS);
    let max_per_tick = state.substrate.mood.modulation().arbiter_max_per_tick;

    let mut reduced: Vec<Intervention> = Vec::new();
    for (name, prop) in raw {
        // Per-(arbiter, target) cooldown: skip if an InterventionEmitted
        // signal exists for this (name, target) in the cooldown window.
        let already = state.substrate.signals.values().any(|s| {
            s.kind == SignalKind::InterventionEmitted
                && s.posted_by == format!("arbiter:{name}")
                && s.posted_at_gen >= cooldown_start
                && arbiter_target_matches_signal(&prop.target, &s.target)
        });
        if already {
            continue;
        }

        // Downgrade if retry budget exhausted.
        let kind = if state.genome_retry_count >= genome_retry_limit {
            InterventionKind::EmitSignalOnly
        } else {
            prop.kind.clone()
        };

        reduced.push(Intervention {
            arbiter_name: name,
            kind,
            target: prop.target,
            rationale: prop.rationale,
            payload: prop.payload,
            decided_at_gen: current_gen,
        });

        if reduced.len() >= max_per_tick {
            break;
        }
    }
    reduced
}

/// Helper: map an InterventionTarget to a SignalTarget for emission +
/// cooldown matching.
pub fn intervention_to_signal_target(t: &InterventionTarget) -> SignalTarget {
    match t {
        InterventionTarget::Agent { agent_id }
        | InterventionTarget::AgentPair { a: agent_id, .. } => SignalTarget::Agent {
            agent_id: agent_id.clone(),
        },
        InterventionTarget::Mission { .. } | InterventionTarget::Global => SignalTarget::Global,
    }
}

fn arbiter_target_matches_signal(arb: &InterventionTarget, sig: &SignalTarget) -> bool {
    match (arb, sig) {
        (InterventionTarget::Agent { agent_id: a }, SignalTarget::Agent { agent_id: b }) => a == b,
        (InterventionTarget::AgentPair { a, b }, SignalTarget::Agent { agent_id: x }) => {
            a == x || b == x
        }
        (InterventionTarget::Global, SignalTarget::Global)
        | (InterventionTarget::Mission { .. }, SignalTarget::Global) => true,
        _ => false,
    }
}

/// Emit the InterventionEmitted signal for each reduced intervention
/// and push them onto the queue for nit-tui to drain.
pub fn apply_interventions(state: &mut AppState, interventions: Vec<Intervention>) {
    for iv in interventions {
        let signal_target = intervention_to_signal_target(&iv.target);
        let posted_by = format!("arbiter:{}", iv.arbiter_name);
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
                "rationale": iv.rationale,
                "target": match &iv.target {
                    InterventionTarget::Agent { agent_id } => serde_json::json!({"agent": agent_id}),
                    InterventionTarget::AgentPair { a, b } => serde_json::json!({"pair": [a, b]}),
                    InterventionTarget::Mission { mission_id } => serde_json::json!({"mission": mission_id}),
                    InterventionTarget::Global => serde_json::json!({"scope": "global"}),
                },
                "details": iv.payload.clone(),
            }),
        });
        state.pending_interventions.push(iv);
    }
}

#[cfg(test)]
#[path = "../tests/arbiters.rs"]
mod tests;
