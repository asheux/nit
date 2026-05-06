//! HelpNeeded for planners that emit ≥THRESHOLD `reason="unresolved_dep"`
//! Warnings within WINDOW_GENS, with self-silencing on the planner agent.

use std::collections::HashMap;

use super::{ObservedEmission, Observer, OBSERVER_INITIAL_STRENGTH};
use crate::state::AppState;
use crate::substrate::{SignalKind, SignalTarget};

pub const OBSERVER: Observer = Observer {
    name: "sparse_plan",
    run: observe,
};

const WINDOW_GENS: u64 = 10;
const THRESHOLD: usize = 3;

fn observe(state: &AppState) -> Vec<ObservedEmission> {
    let sub = &state.substrate;
    let window_start = sub.current_generation().saturating_sub(WINDOW_GENS);

    let mut by_planner: HashMap<String, (usize, Vec<String>)> = HashMap::new();
    for signal in super::iter_recent_warnings(sub, window_start) {
        if !signal.posted_by.starts_with("planner:") {
            continue;
        }
        if signal.payload.get("reason").and_then(|v| v.as_str()) != Some("unresolved_dep") {
            continue;
        }
        let missing = signal
            .payload
            .get("missing_dep")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let entry = by_planner
            .entry(signal.posted_by.clone())
            .or_insert((0, Vec::new()));
        entry.0 += 1;
        if !missing.is_empty() && !entry.1.contains(&missing) {
            entry.1.push(missing);
        }
    }

    let recent_helps = super::recent_help_targets(sub, "observer:sparse_plan", window_start);

    let mut out = Vec::new();
    for (planner_posted_by, (count, missing_deps)) in by_planner {
        if count < THRESHOLD {
            continue;
        }
        let agent_id = planner_posted_by
            .strip_prefix("planner:")
            .unwrap_or(&planner_posted_by)
            .to_string();
        if recent_helps.contains(&agent_id) {
            continue;
        }
        out.push(ObservedEmission {
            kind: SignalKind::HelpNeeded,
            target: SignalTarget::Agent {
                agent_id: agent_id.clone(),
            },
            initial_strength: OBSERVER_INITIAL_STRENGTH,
            payload: serde_json::json!({
                "reason": "sparse_plan",
                "planner": agent_id,
                "unresolved_count": count,
                "window_gens": WINDOW_GENS,
                "missing_deps_sample": missing_deps,
            }),
        });
    }
    out
}
