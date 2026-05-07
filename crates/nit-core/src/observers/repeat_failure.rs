//! HelpNeeded for an agent accumulating ≥threshold Warnings within a
//! sliding window. Threshold is mood-driven (relaxes/tightens with system
//! mood via `state.substrate.mood.modulation().repeat_failure_threshold`).

use super::{ObservedEmission, Observer};
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

    let warnings_by_agent =
        super::count_by_key(super::iter_recent_warnings(sub, window_start), |s| {
            s.posted_by.clone()
        });

    let recent_helps = super::recent_help_targets(sub, "observer:repeat_failure", window_start);
    let threshold = sub.mood.modulation().repeat_failure_threshold;

    let mut emissions = Vec::new();
    for (agent_id, count) in warnings_by_agent {
        if count < threshold || recent_helps.contains(&agent_id) {
            continue;
        }
        emissions.push(ObservedEmission::new(
            SignalKind::HelpNeeded,
            SignalTarget::Agent {
                agent_id: agent_id.clone(),
            },
            serde_json::json!({
                "reason": "repeat_failure",
                "warning_count": count,
                "window_gens": WINDOW_GENS,
                "agent_id": agent_id,
            }),
        ));
    }
    emissions
}
