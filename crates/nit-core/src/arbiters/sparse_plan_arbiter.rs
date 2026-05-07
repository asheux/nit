//! Re-prompt planners whose `observer:sparse_plan` HelpNeeded carries an
//! unresolved-deps sample, surfacing that sample in the prompt so the
//! planner can self-correct missing task ids.

use super::{Arbiter, InterventionKind, InterventionProposal, InterventionTarget};
use crate::state::AppState;
use crate::substrate::Signal;

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
        build_sparse_plan_proposal,
    )
}

fn build_sparse_plan_proposal(signal: &Signal, agent_id: &str) -> InterventionProposal {
    let missing_deps = missing_deps_csv(signal);
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
}

fn missing_deps_csv(signal: &Signal) -> String {
    signal
        .payload
        .get("missing_deps_sample")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn format_sparse_plan_prompt(missing_deps: &str) -> String {
    format!(
        "ARBITER: your recent plans repeatedly reference task IDs that don't exist in the DAG (missing: {missing_deps}). \
Re-examine the task ids in your output. Every `deps` entry must match an id from your tasks list. \
If you intended a role phase that the template doesn't support (e.g. judge in parallel), omit it and wire integrators directly to the proposer outputs."
    )
}
