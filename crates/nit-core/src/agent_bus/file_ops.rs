use std::path::Path;

use crate::state::{AppState, ClaimRetryRequest};

pub(super) fn handle_file_write(
    state: &mut AppState,
    agent_id: &str,
    mission_id: &Option<String>,
    path: &Path,
) {
    state
        .genome_turn_modified
        .entry(agent_id.to_string())
        .or_default()
        .insert(path.to_path_buf());
    // Mission-scoped accumulator so a reviewer running at swarm end can see
    // every file touched during the mission, even when the same agent's
    // per-turn set was cleared between sequential tasks. Prefer the
    // mission_id the runner sent (eliminates the race with TurnStarted);
    // fall back to the agent's `current_mission` for legacy emitters that
    // don't carry it yet.
    let mission = mission_id.clone().or_else(|| {
        state
            .agents
            .agents
            .iter()
            .find(|a| a.id == agent_id)
            .and_then(|a| a.current_mission.clone())
    });
    if let Some(mission) = mission {
        state
            .genome_mission_modified
            .entry(mission)
            .or_default()
            .insert(path.to_path_buf());
    }

    auto_claim_file(state, agent_id, path);
    invalidate_assumptions(state, agent_id, path);
}

// Mood v2: the base 3-gen TTL is scaled by the mood's claim TTL multiplier
// (Defensive 1.5x, Exploration 0.75x), clamped to a minimum of 1 gen so claims
// can't auto-expire the instant they are created.
fn auto_claim_file(state: &mut AppState, agent_id: &str, path: &Path) {
    const BASE_CLAIM_TTL_GENS: u64 = 3;
    let current_gen = state.substrate.current_generation();
    let ttl_multiplier = state.substrate.mood.modulation().claim_ttl_multiplier;
    let ttl_gens = ((BASE_CLAIM_TTL_GENS as f32) * ttl_multiplier).max(1.0) as u64;
    let claim_id = state.substrate.next_claim_id(agent_id);
    let claim = crate::substrate::Claim {
        id: claim_id,
        kind: crate::substrate::ClaimKind::ExclusiveWrite,
        target: crate::substrate::ClaimTarget::File {
            path: path.to_path_buf(),
        },
        claimed_by: agent_id.to_string(),
        claimed_at_gen: current_gen,
        ttl_gens,
        rationale: "auto-claim from FileWrite".to_string(),
    };
    let Err(conflict) = state.substrate.assert_claim(claim) else {
        return;
    };
    for existing in &conflict.conflicts {
        let id = state.substrate.next_signal_id(agent_id);
        let posted_at_gen = state.substrate.current_generation();
        state.substrate.emit_signal(crate::substrate::Signal {
            id,
            kind: crate::substrate::SignalKind::ClaimViolation,
            posted_by: agent_id.to_string(),
            posted_at_gen,
            target: crate::substrate::SignalTarget::Agent {
                agent_id: agent_id.to_string(),
            },
            initial_strength: crate::substrate::SubstrateState::DEFAULT_INITIAL_STRENGTH,
            payload: serde_json::json!({
                "path": path,
                "attempted_kind": "exclusive_write",
                "conflicting_holder": existing.claimed_by,
                "conflicting_kind": format!("{:?}", existing.kind),
                "conflicting_rationale": existing.rationale,
            }),
        });
    }
    if let Some(first) = conflict.conflicts.first() {
        state.pending_claim_retries.push(ClaimRetryRequest {
            agent_id: agent_id.to_string(),
            path: path.to_path_buf(),
            conflicting_holder: first.claimed_by.clone(),
            conflicting_kind: format!("{:?}", first.kind),
            conflicting_rationale: first.rationale.clone(),
        });
    }
}

// Phase 4: invalidate assumptions whose target overlaps the written path.
// Runs after the auto-claim block so it fires whether the auto-claim succeeded
// or conflicted — the write hit disk either way.
fn invalidate_assumptions(state: &mut AppState, agent_id: &str, path: &Path) {
    let invalidated = state.substrate.invalidate_assumptions_for_write(path);
    for gone in invalidated {
        let id = state.substrate.next_signal_id(agent_id);
        let posted_at_gen = state.substrate.current_generation();
        let assumption_value = serde_json::to_value(&gone).unwrap_or(serde_json::Value::Null);
        state.substrate.emit_signal(crate::substrate::Signal {
            id,
            kind: crate::substrate::SignalKind::Warning,
            posted_by: agent_id.to_string(),
            posted_at_gen,
            target: crate::substrate::SignalTarget::Agent {
                agent_id: gone.posted_by.clone(),
            },
            initial_strength: crate::substrate::SubstrateState::DEFAULT_INITIAL_STRENGTH,
            payload: serde_json::json!({
                "reason": "assumption_invalidated_by_write",
                "written_path": path,
                "writer": agent_id,
                "assumption": assumption_value,
            }),
        });
    }
}
