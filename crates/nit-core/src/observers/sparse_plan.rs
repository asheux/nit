//! SparsePlanObserver — count `reason="unresolved_dep"` Warnings grouped by
//! planner posted_by ("planner:{agent_id}") within a 10-generation window.
//! When count ≥ 3, emit HelpNeeded targeting the planner. Self-silences if
//! a recent `observer:sparse_plan` HelpNeeded already targets the planner.

use std::collections::{HashMap, HashSet};

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
    let current_gen = sub.current_generation();
    let window_start = current_gen.saturating_sub(WINDOW_GENS);

    // Group unresolved-dep warnings by planner:agent_id posted_by.
    let mut by_planner: HashMap<String, (usize, Vec<String>)> = HashMap::new();
    for signal in sub.signals.values() {
        if signal.kind != SignalKind::Warning || signal.posted_at_gen < window_start {
            continue;
        }
        if !signal.posted_by.starts_with("planner:") {
            continue;
        }
        let is_unresolved =
            signal.payload.get("reason").and_then(|v| v.as_str()) == Some("unresolved_dep");
        if !is_unresolved {
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

    // Self-silencing: skip if a recent sparse_plan HelpNeeded already
    // targets this planner.
    let mut recent_helps: HashSet<String> = HashSet::new();
    for signal in sub.signals.values() {
        if signal.kind == SignalKind::HelpNeeded
            && signal.posted_by == "observer:sparse_plan"
            && signal.posted_at_gen >= window_start
        {
            if let SignalTarget::Agent { agent_id } = &signal.target {
                recent_helps.insert(agent_id.clone());
            }
        }
    }

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
