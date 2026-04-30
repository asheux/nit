//! PersistentConflictArbiter: detects mutual claim-conflict oscillation
//! between agent pairs and escalates with a "permanently yield" prompt.

use std::collections::HashMap;

use super::{Arbiter, InterventionKind, InterventionProposal, InterventionTarget};
use crate::state::AppState;
use crate::substrate::SignalKind;

pub const ARBITER: Arbiter = Arbiter {
    name: "persistent_conflict",
    run: observe,
};

const CONFLICT_WINDOW_GENS: u64 = 10;
const CONFLICT_THRESHOLD: usize = 3;

fn observe(state: &AppState) -> Vec<InterventionProposal> {
    let sub = &state.substrate;
    let current_gen = sub.current_generation();
    let window_start = current_gen.saturating_sub(CONFLICT_WINDOW_GENS);

    // Collect all ClaimViolation signals in the window, extracting
    // (violator, holder, path) from the payload.  Key is normalized pair
    // (alphabetically sorted) so A<->B is counted once.
    let mut pair_counts: HashMap<(String, String), (usize, Vec<String>)> = HashMap::new();

    for s in sub.signals.values() {
        if s.kind != SignalKind::ClaimViolation {
            continue;
        }
        if s.posted_at_gen < window_start {
            continue;
        }
        let violator = s.posted_by.clone();
        let holder = s
            .payload
            .get("conflicting_holder")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let path = s
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        if let Some(h) = holder {
            if violator == h {
                continue;
            }
            let pair = if violator < h {
                (violator.clone(), h.clone())
            } else {
                (h.clone(), violator.clone())
            };
            let entry = pair_counts.entry(pair).or_insert((0, Vec::new()));
            entry.0 += 1;
            if !entry.1.contains(&path) && !path.is_empty() {
                entry.1.push(path);
            }
        }
    }

    let mut proposals = Vec::new();
    for ((a, b), (count, paths)) in pair_counts {
        if count < CONFLICT_THRESHOLD {
            continue;
        }
        // Pick the target as lexicographically-larger agent for deterministic tiebreak.
        let (target_agent, other) = if a > b {
            (a.clone(), b.clone())
        } else {
            (b.clone(), a.clone())
        };
        let paths_list = if paths.is_empty() {
            "shared resources".to_string()
        } else {
            paths.join(", ")
        };
        let prompt = format!(
            "ARBITER: you and {other} have conflicted on {paths_list} {count} times in {CONFLICT_WINDOW_GENS} generations. You must permanently yield this resource for this mission. Choose a different file or coordinate through an explicit artifact."
        );
        proposals.push(InterventionProposal {
            kind: InterventionKind::RedispatchWithEscalatedPrompt { prompt },
            target: InterventionTarget::AgentPair {
                a: a.clone(),
                b: b.clone(),
            },
            rationale: format!(
                "persistent-conflict: {a} and {b} ({count} violations in {CONFLICT_WINDOW_GENS}g)"
            ),
            payload: serde_json::json!({
                "violator_pair": [a, b],
                "chosen_recipient": target_agent,
                "violation_count": count,
                "window_gens": CONFLICT_WINDOW_GENS,
                "contested_paths": paths,
            }),
        });
    }
    proposals
}
