use crate::state::AppState;
use crate::substrate::{
    Assumption, AssumptionTarget, Claim, ClaimConflict, ClaimKind, ClaimTarget, Signal, SignalKind,
    SignalTarget, SubstrateState,
};

pub(super) fn handle_assert_claim(state: &mut AppState, claim: &Claim) {
    let Err(conflict) = state.substrate.assert_claim(claim.clone()) else {
        return;
    };
    let attempted_kind = format!("{:?}", claim.kind);
    emit_violation_signals(state, &claim.claimed_by, &attempted_kind, &conflict, false);
}

pub(super) fn handle_emit_signal_request(
    state: &mut AppState,
    posted_by: &str,
    kind: SignalKind,
    target: &SignalTarget,
    payload: &serde_json::Value,
    initial_strength: Option<f32>,
) {
    let id = state.substrate.next_signal_id(posted_by);
    let posted_at_gen = state.substrate.current_generation();
    state.substrate.emit_signal(Signal {
        id,
        kind,
        posted_by: posted_by.to_string(),
        posted_at_gen,
        target: target.clone(),
        initial_strength: initial_strength.unwrap_or(SubstrateState::DEFAULT_INITIAL_STRENGTH),
        payload: payload.clone(),
    });
}

// Mirrors the `FileWrite` auto-claim: TTL is mood-scaled and clamped to a
// minimum of 1 gen. Conflicts surface as ClaimViolation signals targeted at
// the requester (one signal per conflicting existing claim).
pub(super) fn handle_assert_claim_request(
    state: &mut AppState,
    claimed_by: &str,
    kind: ClaimKind,
    target: &ClaimTarget,
    ttl_gens: u64,
    rationale: &str,
) {
    let id = state.substrate.next_claim_id(claimed_by);
    let claim = build_request_claim(state, id, claimed_by, kind, target, ttl_gens, rationale);
    let Err(conflict) = state.substrate.assert_claim(claim) else {
        return;
    };
    let attempted_kind = format!("{kind:?}");
    emit_violation_signals(state, claimed_by, &attempted_kind, &conflict, true);
}

pub(super) fn handle_assert_assumption_request(
    state: &mut AppState,
    posted_by: &str,
    target: &AssumptionTarget,
    fact: &serde_json::Value,
    ttl_gens: u64,
    rationale: &str,
) {
    let assumption = Assumption {
        id: state.substrate.next_assumption_id(posted_by),
        target: target.clone(),
        fact: fact.clone(),
        posted_by: posted_by.to_string(),
        posted_at_gen: state.substrate.current_generation(),
        ttl_gens,
        rationale: rationale.to_string(),
    };
    state.substrate.assert_assumption(assumption);
}

fn build_request_claim(
    state: &AppState,
    id: crate::substrate::ClaimId,
    claimed_by: &str,
    kind: ClaimKind,
    target: &ClaimTarget,
    ttl_gens: u64,
    rationale: &str,
) -> Claim {
    let ttl_multiplier = state.substrate.mood.modulation().claim_ttl_multiplier;
    let adjusted_ttl = ((ttl_gens as f32) * ttl_multiplier).max(1.0) as u64;
    Claim {
        id,
        kind,
        target: target.clone(),
        claimed_by: claimed_by.to_string(),
        claimed_at_gen: state.substrate.current_generation(),
        ttl_gens: adjusted_ttl,
        rationale: rationale.to_string(),
    }
}

// `with_reason=true` adds the `assert_claim_request_conflict` reason field
// to disambiguate request-mode violations from FileWrite/AssertClaim ones.
fn emit_violation_signals(
    state: &mut AppState,
    requester: &str,
    attempted_kind: &str,
    conflict: &ClaimConflict,
    with_reason: bool,
) {
    for existing in &conflict.conflicts {
        let id = state.substrate.next_signal_id(requester);
        let posted_at_gen = state.substrate.current_generation();
        let payload = build_violation_payload(attempted_kind, existing, with_reason);
        state.substrate.emit_signal(Signal {
            id,
            kind: SignalKind::ClaimViolation,
            posted_by: requester.to_string(),
            posted_at_gen,
            target: SignalTarget::Agent {
                agent_id: requester.to_string(),
            },
            initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
            payload,
        });
    }
}

fn build_violation_payload(
    attempted_kind: &str,
    existing: &Claim,
    with_reason: bool,
) -> serde_json::Value {
    let conflicting_kind = format!("{:?}", existing.kind);
    if with_reason {
        return serde_json::json!({
            "reason": "assert_claim_request_conflict",
            "attempted_kind": attempted_kind,
            "conflicting_holder": existing.claimed_by,
            "conflicting_kind": conflicting_kind,
            "conflicting_rationale": existing.rationale,
        });
    }
    serde_json::json!({
        "attempted_kind": attempted_kind,
        "conflicting_holder": existing.claimed_by,
        "conflicting_kind": conflicting_kind,
    })
}
