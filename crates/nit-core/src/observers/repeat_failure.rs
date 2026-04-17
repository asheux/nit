//! RepeatFailureObserver — if an agent accumulates ≥2 Warning signals in
//! the last 5 generations AND no recent observer-emitted HelpNeeded already
//! targets them, emit HelpNeeded on that agent.

use std::collections::HashMap;

use super::{ObservedEmission, Observer, OBSERVER_INITIAL_STRENGTH};
use crate::state::AppState;
use crate::substrate::{SignalKind, SignalTarget};

pub const OBSERVER: Observer = Observer {
    name: "repeat_failure",
    run: observe,
};

const WINDOW_GENS: u64 = 5;
const THRESHOLD: usize = 2;

fn observe(state: &AppState) -> Vec<ObservedEmission> {
    let sub = &state.substrate;
    let current_gen = sub.current_generation();
    let window_start = current_gen.saturating_sub(WINDOW_GENS);

    // Count recent Warnings per posted_by.
    let mut warnings_by_agent: HashMap<String, usize> = HashMap::new();
    for s in sub.signals.values() {
        if s.kind == SignalKind::Warning && s.posted_at_gen >= window_start {
            *warnings_by_agent.entry(s.posted_by.clone()).or_insert(0) += 1;
        }
    }

    // Self-silencing: skip if a recent observer:repeat_failure HelpNeeded
    // already targets this agent.
    let mut recent_helps: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in sub.signals.values() {
        if s.kind == SignalKind::HelpNeeded
            && s.posted_by == "observer:repeat_failure"
            && s.posted_at_gen >= window_start
        {
            if let SignalTarget::Agent { agent_id } = &s.target {
                recent_helps.insert(agent_id.clone());
            }
        }
    }

    let mut emissions = Vec::new();
    for (agent_id, count) in warnings_by_agent {
        if count < THRESHOLD {
            continue;
        }
        if recent_helps.contains(&agent_id) {
            continue;
        }
        emissions.push(ObservedEmission {
            kind: SignalKind::HelpNeeded,
            target: SignalTarget::Agent {
                agent_id: agent_id.clone(),
            },
            initial_strength: OBSERVER_INITIAL_STRENGTH,
            payload: serde_json::json!({
                "reason": "repeat_failure",
                "warning_count": count,
                "window_gens": WINDOW_GENS,
                "agent_id": agent_id,
            }),
        });
    }
    emissions
}
