//! HelpNeededArbiter — when `observer:repeat_failure` emits a HelpNeeded
//! targeting an agent, propose a RedispatchWithEscalatedPrompt that tells
//! the agent to downscope, simplify, and surface the blocker in plain text
//! instead of retrying the same failing approach.
//!
//! Cooldown + per-tick budget are enforced by `reduce_proposals`, so this
//! arbiter is free to emit one proposal per failing agent per tick without
//! risk of a retry storm.

use std::collections::HashSet;

use super::{Arbiter, InterventionKind, InterventionProposal, InterventionTarget};
use crate::state::AppState;
use crate::substrate::{SignalKind, SignalTarget};

pub const ARBITER: Arbiter = Arbiter {
    name: "help_needed",
    run: observe,
};

const WINDOW_GENS: u64 = 5;

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
        if signal.kind != SignalKind::HelpNeeded || signal.posted_by != "observer:repeat_failure" {
            continue;
        }
        let SignalTarget::Agent { agent_id } = &signal.target else {
            continue;
        };
        if !seen_agents.insert(agent_id.clone()) {
            continue;
        }
        let warning_count = signal
            .payload
            .get("warning_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let prompt = format!(
            "ARBITER: you have failed {warning_count} times in the last {WINDOW_GENS} generations. \
Stop retrying the same approach. Before writing anything: (1) state the smallest concrete blocker in one sentence, \
(2) downscope — pick the minimum viable slice of the task you can actually complete, \
(3) if the blocker is context size or input volume, summarize aggressively before acting. \
Do not emit more than one tool call before posting your downscoped plan as text."
        );
        proposals.push(InterventionProposal {
            kind: InterventionKind::RedispatchWithEscalatedPrompt { prompt },
            target: InterventionTarget::Agent {
                agent_id: agent_id.clone(),
            },
            rationale: format!(
                "help_needed: agent {agent_id} has {warning_count} warnings in {WINDOW_GENS}g"
            ),
            payload: serde_json::json!({
                "agent": agent_id,
                "warning_count": warning_count,
            }),
        });
    }
    proposals
}
