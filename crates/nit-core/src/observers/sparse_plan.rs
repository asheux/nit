//! HelpNeeded for planners that emit ≥`THRESHOLD` `reason="unresolved_dep"`
//! Warnings within `WINDOW_GENS`, with self-silencing on the planner agent.

use super::{ObservedEmission, Observer};
use crate::state::AppState;
use crate::substrate::{Signal, SignalKind, SignalTarget};

pub const OBSERVER: Observer = Observer {
    name: "sparse_plan",
    run: observe,
};

const WINDOW_GENS: u64 = 10;
const THRESHOLD: usize = 3;
const PLANNER_PREFIX: &str = "planner:";

fn observe(state: &AppState) -> Vec<ObservedEmission> {
    let sub = &state.substrate;
    let window_start = sub.current_generation().saturating_sub(WINDOW_GENS);

    let by_planner = super::count_with_unique(
        super::iter_recent_warnings(sub, window_start),
        planner_posted_by_for_unresolved_dep,
        |signal| super::payload_string(&signal.payload, "missing_dep"),
    );

    let already_alerted = super::recent_help_targets(sub, "observer:sparse_plan", window_start);

    by_planner
        .into_iter()
        .filter(|(_, (count, _))| *count >= THRESHOLD)
        .filter_map(|(planner_posted_by, (count, missing_deps))| {
            let agent_id = planner_posted_by
                .strip_prefix(PLANNER_PREFIX)
                .unwrap_or(&planner_posted_by)
                .to_string();
            if already_alerted.contains(&agent_id) {
                return None;
            }
            Some(ObservedEmission::new(
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
            ))
        })
        .collect()
}

// Planners post warnings under `posted_by = "planner:<agent_id>"` and we only
// count the ones tagged `reason = "unresolved_dep"`. Returning the full
// `posted_by` as the bucket key (rather than the stripped agent_id) preserves
// the prefix for downstream filtering and keeps behaviour stable if an agent
// ever shares an id with a non-planner role.
fn planner_posted_by_for_unresolved_dep(signal: &Signal) -> Option<String> {
    if !signal.posted_by.starts_with(PLANNER_PREFIX) {
        return None;
    }
    if signal.payload.get("reason").and_then(|v| v.as_str()) != Some("unresolved_dep") {
        return None;
    }
    Some(signal.posted_by.clone())
}
