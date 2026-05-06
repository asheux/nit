//! HelpNeeded for an agent that accumulates ≥threshold Warning signals in
//! the last WINDOW_GENS generations. Threshold is read from
//! `state.substrate.mood.modulation().repeat_failure_threshold` so the policy
//! tightens or relaxes with the system mood.

use std::collections::HashMap;

use super::{ObservedEmission, Observer, OBSERVER_INITIAL_STRENGTH};
use crate::state::AppState;
use crate::substrate::{SignalKind, SignalTarget};

pub const OBSERVER: Observer = Observer {
    name: "repeat_failure",
    run: observe,
};

const WINDOW_GENS: u64 = 5;

fn observe(state: &AppState) -> Vec<ObservedEmission> {
    let sub = &state.substrate;
    let window_start = sub.current_generation().saturating_sub(WINDOW_GENS);

    let mut warnings_by_agent: HashMap<String, usize> = HashMap::new();
    for s in super::iter_recent_warnings(sub, window_start) {
        *warnings_by_agent.entry(s.posted_by.clone()).or_insert(0) += 1;
    }

    let recent_helps = super::recent_help_targets(sub, "observer:repeat_failure", window_start);
    let threshold = sub.mood.modulation().repeat_failure_threshold;

    let mut emissions = Vec::new();
    for (agent_id, count) in warnings_by_agent {
        if count < threshold || recent_helps.contains(&agent_id) {
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
