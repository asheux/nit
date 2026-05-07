//! HelpNeeded for planners that emit ≥THRESHOLD `reason="unresolved_dep"`
//! Warnings within WINDOW_GENS, with self-silencing on the planner agent.

use super::{ObservedEmission, Observer};
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

    let by_planner = super::count_with_unique(
        super::iter_recent_warnings(sub, window_start),
        |signal| {
            if !signal.posted_by.starts_with("planner:") {
                return None;
            }
            if signal.payload.get("reason").and_then(|v| v.as_str()) != Some("unresolved_dep") {
                return None;
            }
            Some(signal.posted_by.clone())
        },
        |signal| super::payload_string(&signal.payload, "missing_dep"),
    );

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
        out.push(ObservedEmission::new(
            SignalKind::HelpNeeded,
            SignalTarget::Agent {
                agent_id: agent_id.clone(),
            },
            serde_json::json!({
                "reason": "sparse_plan",
                "planner": agent_id,
                "unresolved_count": count,
                "window_gens": WINDOW_GENS,
                "missing_deps_sample": missing_deps,
            }),
        ));
    }
    out
}
