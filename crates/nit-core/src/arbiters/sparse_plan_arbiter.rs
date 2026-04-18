//! SparsePlanArbiter — when `observer:sparse_plan` emits a HelpNeeded
//! targeting a planner, propose a RedispatchWithEscalatedPrompt carrying
//! the observed missing-deps sample so the planner can self-correct.

use std::collections::HashSet;

use super::{Arbiter, InterventionKind, InterventionProposal, InterventionTarget};
use crate::state::AppState;
use crate::substrate::{SignalKind, SignalTarget};

pub const ARBITER: Arbiter = Arbiter {
    name: "sparse_plan",
    run: observe,
};

const WINDOW_GENS: u64 = 10;

fn observe(state: &AppState) -> Vec<InterventionProposal> {
    let sub = &state.substrate;
    let current_gen = sub.current_generation();
    let window_start = current_gen.saturating_sub(WINDOW_GENS);

    let mut proposals = Vec::new();
    let mut seen_agents: HashSet<String> = HashSet::new();
    for signal in sub.signals.values() {
        if signal.posted_at_gen < window_start {
            continue;
        }
        if signal.kind != SignalKind::HelpNeeded || signal.posted_by != "observer:sparse_plan" {
            continue;
        }
        let SignalTarget::Agent { agent_id } = &signal.target else {
            continue;
        };
        if !seen_agents.insert(agent_id.clone()) {
            continue;
        }
        let missing_deps = signal
            .payload
            .get("missing_deps_sample")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let prompt = format!(
            "ARBITER: your recent plans repeatedly reference task IDs that don't exist in the DAG (missing: {missing_deps}). \
Re-examine the task ids in your output. Every `deps` entry must match an id from your tasks list. \
If you intended a role phase that the template doesn't support (e.g. judge in parallel), omit it and wire integrators directly to the proposer outputs."
        );
        proposals.push(InterventionProposal {
            kind: InterventionKind::RedispatchWithEscalatedPrompt { prompt },
            target: InterventionTarget::Agent {
                agent_id: agent_id.clone(),
            },
            rationale: format!(
                "sparse_plan: planner {agent_id} has {} unresolved-dep warnings in {WINDOW_GENS}g",
                signal
                    .payload
                    .get("unresolved_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            ),
            payload: serde_json::json!({
                "planner": agent_id,
                "missing_deps_sample": signal.payload.get("missing_deps_sample"),
            }),
        });
    }
    proposals
}
