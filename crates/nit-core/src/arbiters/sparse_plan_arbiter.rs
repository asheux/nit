//! SparsePlanArbiter — when `observer:sparse_plan` emits a HelpNeeded
//! targeting a planner, propose a RedispatchWithEscalatedPrompt carrying
//! the observed missing-deps sample so the planner can self-correct.

use super::{Arbiter, InterventionKind, InterventionProposal, InterventionTarget};
use crate::state::AppState;

pub const ARBITER: Arbiter = Arbiter {
    name: "sparse_plan",
    run: observe,
};

const WINDOW_GENS: u64 = 10;

fn observe(state: &AppState) -> Vec<InterventionProposal> {
    super::scan_help_needed_signals(
        state,
        "observer:sparse_plan",
        WINDOW_GENS,
        |signal, agent_id| {
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
            let unresolved_count = signal
                .payload
                .get("unresolved_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            InterventionProposal {
            kind: InterventionKind::RedispatchWithEscalatedPrompt {
                prompt: format_sparse_plan_prompt(&missing_deps),
            },
            target: InterventionTarget::Agent {
                agent_id: agent_id.to_string(),
            },
            rationale: format!(
                "sparse_plan: planner {agent_id} has {unresolved_count} unresolved-dep warnings in {WINDOW_GENS}g"
            ),
            payload: serde_json::json!({
                "planner": agent_id,
                "missing_deps_sample": signal.payload.get("missing_deps_sample"),
            }),
        }
        },
    )
}

fn format_sparse_plan_prompt(missing_deps: &str) -> String {
    format!(
        "ARBITER: your recent plans repeatedly reference task IDs that don't exist in the DAG (missing: {missing_deps}). \
Re-examine the task ids in your output. Every `deps` entry must match an id from your tasks list. \
If you intended a role phase that the template doesn't support (e.g. judge in parallel), omit it and wire integrators directly to the proposer outputs."
    )
}
