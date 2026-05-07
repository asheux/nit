//! Detect mutual claim-conflict oscillation between agent pairs and escalate
//! with a "permanently yield" prompt aimed at the lexicographically-larger
//! agent so the tiebreak is deterministic across re-runs.

use std::collections::HashMap;

use super::{Arbiter, InterventionKind, InterventionProposal, InterventionTarget};
use crate::state::AppState;
use crate::substrate::{Signal, SignalKind};

pub const ARBITER: Arbiter = Arbiter {
    name: "persistent_conflict",
    run: observe,
};

const CONFLICT_WINDOW_GENS: u64 = 10;
const CONFLICT_THRESHOLD: usize = 3;

fn observe(state: &AppState) -> Vec<InterventionProposal> {
    let sub = &state.substrate;
    let window_start = sub
        .current_generation()
        .saturating_sub(CONFLICT_WINDOW_GENS);

    let mut pair_counts: HashMap<(String, String), (usize, Vec<String>)> = HashMap::new();
    for s in sub.signals.values() {
        if s.kind != SignalKind::ClaimViolation || s.posted_at_gen < window_start {
            continue;
        }
        let Some((violator, holder, path)) = extract_violation(s) else {
            continue;
        };
        let pair = normalize_pair(violator, holder);
        let entry = pair_counts.entry(pair).or_insert((0, Vec::new()));
        entry.0 += 1;
        if !path.is_empty() && !entry.1.contains(&path) {
            entry.1.push(path);
        }
    }

    pair_counts
        .into_iter()
        .filter(|(_, (count, _))| *count >= CONFLICT_THRESHOLD)
        .map(|((a, b), (count, paths))| build_proposal(a, b, count, paths))
        .collect()
}

fn extract_violation(signal: &Signal) -> Option<(String, String, String)> {
    let violator = signal.posted_by.clone();
    let holder = signal
        .payload
        .get("conflicting_holder")
        .and_then(|v| v.as_str())?
        .to_string();
    if violator == holder {
        return None;
    }
    let path = signal
        .payload
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Some((violator, holder, path))
}

fn normalize_pair(a: String, b: String) -> (String, String) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

fn build_proposal(a: String, b: String, count: usize, paths: Vec<String>) -> InterventionProposal {
    // pair is normalized (a < b) — the larger element receives the yield prompt.
    let target_agent = b.clone();
    let other = a.clone();
    InterventionProposal {
        kind: InterventionKind::RedispatchWithEscalatedPrompt {
            prompt: format_persistent_conflict_prompt(&other, &paths, count),
        },
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
    }
}

fn format_persistent_conflict_prompt(other: &str, paths: &[String], count: usize) -> String {
    let paths_list = if paths.is_empty() {
        "shared resources".to_string()
    } else {
        paths.join(", ")
    };
    format!(
        "ARBITER: you and {other} have conflicted on {paths_list} {count} times in {CONFLICT_WINDOW_GENS} generations. You must permanently yield this resource for this mission. Choose a different file or coordinate through an explicit artifact."
    )
}
