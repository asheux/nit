//! HelpNeeded for any agent accumulating ≥threshold Warnings within a sliding
//! window. The threshold is mood-driven (relaxes/tightens with system mood
//! via `state.substrate.mood.modulation().repeat_failure_threshold`) so the
//! observer can stay quiet during exploration and bite under stable mood.

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

    // Self-silence: skip agents we already alerted in this window so the
    // arbiter cooldown is the sole gate on repeat escalation.
    let already_alerted = super::recent_help_targets(sub, "observer:repeat_failure", window_start);
    let threshold = sub.mood.modulation().repeat_failure_threshold;

    warnings_by_agent
        .into_iter()
        .filter(|(agent_id, count)| *count >= threshold && !already_alerted.contains(agent_id))
        .map(|(agent_id, count)| {
            ObservedEmission::new(
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
            )
        })
        .collect()
}
