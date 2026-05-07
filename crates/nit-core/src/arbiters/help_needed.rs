//! Redispatch a downscope-and-explain prompt at agents whose
//! `observer:repeat_failure` HelpNeeded keeps re-firing inside `WINDOW_GENS`.

use super::{Arbiter, InterventionKind, InterventionProposal, InterventionTarget};
use crate::state::AppState;
use crate::substrate::Signal;

pub const ARBITER: Arbiter = Arbiter {
    name: "help_needed",
    run: observe,
};

const WINDOW_GENS: u64 = 5;

fn observe(state: &AppState) -> Vec<InterventionProposal> {
    super::scan_help_needed_signals(
        state,
        "observer:repeat_failure",
        WINDOW_GENS,
        build_help_needed_proposal,
    )
}

fn build_help_needed_proposal(signal: &Signal, agent_id: &str) -> InterventionProposal {
    let warning_count = signal
        .payload
        .get("warning_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    InterventionProposal {
        kind: InterventionKind::RedispatchWithEscalatedPrompt {
            prompt: format_help_needed_prompt(warning_count),
        },
        target: InterventionTarget::Agent {
            agent_id: agent_id.to_string(),
        },
        rationale: format!(
            "help_needed: agent {agent_id} has {warning_count} warnings in {WINDOW_GENS}g"
        ),
        payload: serde_json::json!({
            "agent": agent_id,
            "warning_count": warning_count,
        }),
    }
}

fn format_help_needed_prompt(warning_count: u64) -> String {
    format!(
        "ARBITER: you have failed {warning_count} times in the last {WINDOW_GENS} generations. \
Stop retrying the same approach. Before writing anything: (1) state the smallest concrete blocker in one sentence, \
(2) downscope — pick the minimum viable slice of the task you can actually complete, \
(3) if the blocker is context size or input volume, summarize aggressively before acting. \
Do not emit more than one tool call before posting your downscoped plan as text."
    )
}
