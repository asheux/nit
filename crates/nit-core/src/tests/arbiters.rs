//! Tests for the arbiter framework — observer→arbiter ordering, retry-budget
//! enforcement, and per-arbiter intervention shapes.

use crate::agent_bus::AgentBusEvent;
use crate::arbiters::{
    self, persistent_conflict, reduce_proposals, run_all, sparse_plan_arbiter, InterventionKind,
    InterventionProposal, InterventionTarget, ARBITER_MAX_PER_TICK, ARBITER_RETRY_LIMIT,
};
use crate::state::AppState;
use crate::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};
use crate::test_helpers::{add_codex_agent, temp_dir, test_state, test_state_in};

fn test_state_on_disk(label: &str) -> AppState {
    test_state_in(temp_dir(label))
}

/// Inject a `ClaimViolation` signal mirroring the auto-emit shape from
/// `agent_bus::file_ops` so persistent_conflict sees realistic input.
fn inject_claim_violation(
    state: &mut AppState,
    violator: &str,
    holder: &str,
    path: &str,
    posted_at_gen: u64,
    counter: u64,
) {
    let id = format!("{posted_at_gen}-{violator}-{counter}");
    state.substrate.emit_signal(Signal {
        id,
        kind: SignalKind::ClaimViolation,
        posted_by: violator.into(),
        posted_at_gen,
        target: SignalTarget::Agent {
            agent_id: violator.into(),
        },
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::json!({
            "path": path,
            "attempted_kind": "exclusive_write",
            "conflicting_holder": holder,
            "conflicting_kind": "ExclusiveWrite",
            "conflicting_rationale": "test",
        }),
    });
}

/// Three violations forming the canonical persistent-conflict pattern: two
/// agents fighting over two paths, alternating victim/holder.
fn seed_persistent_conflict_pair(state: &mut AppState) {
    inject_claim_violation(state, "a", "b", "foo.rs", 0, 0);
    inject_claim_violation(state, "b", "a", "foo.rs", 0, 1);
    inject_claim_violation(state, "a", "b", "bar.rs", 0, 2);
}

#[test]
fn framework_invokes_all_registered_arbiters_empty_state() {
    let state = test_state();
    let raw = run_all(&state);
    assert!(raw.is_empty());
    // At least one arbiter is registered so `run_all` really does loop.
    #[allow(clippy::const_is_empty)]
    let registered = !arbiters::REGISTERED_ARBITERS.is_empty();
    assert!(registered);
}

#[test]
fn persistent_conflict_silent_below_threshold() {
    let mut state = test_state();
    inject_claim_violation(&mut state, "a", "b", "foo.rs", 0, 0);
    inject_claim_violation(&mut state, "a", "b", "foo.rs", 0, 1);

    let proposals = (persistent_conflict::ARBITER.run)(&state);
    assert!(proposals.is_empty());
}

#[test]
fn persistent_conflict_triggers_at_threshold() {
    let mut state = test_state();
    seed_persistent_conflict_pair(&mut state);

    let proposals = (persistent_conflict::ARBITER.run)(&state);
    assert_eq!(proposals.len(), 1);
    let prop = &proposals[0];
    match &prop.target {
        InterventionTarget::AgentPair { a, b } => {
            assert_eq!(a, "a");
            assert_eq!(b, "b");
        }
        other => panic!("expected AgentPair target, got {other:?}"),
    }
    match &prop.kind {
        InterventionKind::RedispatchWithEscalatedPrompt { prompt } => {
            assert!(prompt.contains("ARBITER"));
            assert!(prompt.contains("permanently yield"));
        }
        other => panic!("expected RedispatchWithEscalatedPrompt, got {other:?}"),
    }
}

#[test]
fn persistent_conflict_deterministic_tiebreak() {
    let mut state = test_state();
    seed_persistent_conflict_pair(&mut state);

    let proposals = (persistent_conflict::ARBITER.run)(&state);
    assert_eq!(proposals.len(), 1);
    let chosen = proposals[0]
        .payload
        .get("chosen_recipient")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        chosen, "b",
        "lexicographically-larger agent should be chosen",
    );
}

#[test]
fn persistent_conflict_cooldown_respected() {
    let mut state = test_state();
    // Pre-seed an InterventionEmitted for persistent_conflict targeting `a`.
    state.substrate.emit_signal(Signal {
        id: "seed-iv-0".into(),
        kind: SignalKind::InterventionEmitted,
        posted_by: "arbiter:persistent_conflict".into(),
        posted_at_gen: 0,
        target: SignalTarget::Agent {
            agent_id: "a".into(),
        },
        initial_strength: arbiters::ARBITER_INITIAL_STRENGTH,
        payload: serde_json::Value::Null,
    });
    seed_persistent_conflict_pair(&mut state);

    let raw = run_all(&state);
    assert_eq!(raw.len(), 1);
    let reduced = reduce_proposals(&state, raw, ARBITER_RETRY_LIMIT);
    assert!(
        reduced.is_empty(),
        "cooldown should skip the persistent_conflict proposal",
    );
}

#[test]
fn persistent_conflict_outside_window_ignored() {
    let mut state = test_state();
    // Advance past the 10-gen window relative to the violations.
    state.substrate.generation = 20;
    inject_claim_violation(&mut state, "a", "b", "foo.rs", 5, 0);
    inject_claim_violation(&mut state, "b", "a", "foo.rs", 5, 1);
    inject_claim_violation(&mut state, "a", "b", "bar.rs", 5, 2);

    let proposals = (persistent_conflict::ARBITER.run)(&state);
    assert!(proposals.is_empty());
}

#[test]
fn reduce_proposals_enforces_per_tick_budget() {
    let state = test_state();
    let raw: Vec<(&'static str, InterventionProposal)> = (0..(ARBITER_MAX_PER_TICK + 2))
        .map(|i| {
            (
                "persistent_conflict",
                InterventionProposal {
                    kind: InterventionKind::RedispatchWithEscalatedPrompt { prompt: "p".into() },
                    target: InterventionTarget::Agent {
                        agent_id: format!("a{i}"),
                    },
                    rationale: "t".into(),
                    payload: serde_json::Value::Null,
                },
            )
        })
        .collect();

    let reduced = reduce_proposals(&state, raw, ARBITER_RETRY_LIMIT);
    assert_eq!(reduced.len(), ARBITER_MAX_PER_TICK);
}

#[test]
fn turn_completed_integration_queues_intervention() {
    let mut state = test_state_on_disk("integration-turn-completed");
    add_codex_agent(&mut state, "gpt-test");
    seed_persistent_conflict_pair(&mut state);

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "ok".into(),
    }
    .apply(&mut state);

    assert!(
        !state.pending_interventions.is_empty(),
        "expected at least one intervention queued, got {:?}",
        state.pending_interventions,
    );

    let arb_signals: Vec<_> = state
        .substrate
        .signals
        .values()
        .filter(|s| {
            s.kind == SignalKind::InterventionEmitted
                && s.posted_by == "arbiter:persistent_conflict"
        })
        .collect();
    assert!(
        !arb_signals.is_empty(),
        "expected an InterventionEmitted signal from persistent_conflict",
    );
}

#[test]
fn metabolism_tick_runs_arbiters() {
    let mut state = test_state_on_disk("integration-metabolism");
    seed_persistent_conflict_pair(&mut state);

    let outcome = crate::metabolism::tick(&mut state);

    assert!(
        outcome.arbiter_interventions > 0,
        "expected arbiter interventions in tick outcome, got {outcome:?}",
    );
    assert!(
        !state.pending_interventions.is_empty(),
        "expected queue populated after metabolism tick",
    );
}

#[test]
fn intervention_downgrades_to_signal_only_when_retry_budget_exhausted() {
    let mut state = test_state();
    // AgentPair downgrade requires both agents to be exhausted.
    state
        .genome_retry_counts
        .insert("a".to_string(), ARBITER_RETRY_LIMIT);
    state
        .genome_retry_counts
        .insert("b".to_string(), ARBITER_RETRY_LIMIT);
    seed_persistent_conflict_pair(&mut state);

    let raw = run_all(&state);
    assert_eq!(raw.len(), 1);
    let reduced = reduce_proposals(&state, raw, ARBITER_RETRY_LIMIT);
    assert_eq!(reduced.len(), 1);
    match &reduced[0].kind {
        InterventionKind::EmitSignalOnly => {}
        other => panic!("expected EmitSignalOnly, got {other:?}"),
    }
}

#[test]
fn sparse_plan_arbiter_escalates_on_help_needed() {
    let mut state = test_state();
    state.substrate.emit_signal(Signal {
        id: "0-observer:sparse_plan-0".into(),
        kind: SignalKind::HelpNeeded,
        posted_by: "observer:sparse_plan".into(),
        posted_at_gen: 0,
        target: SignalTarget::Agent {
            agent_id: "alice".into(),
        },
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::json!({
            "reason": "sparse_plan",
            "planner": "alice",
            "unresolved_count": 3,
            "missing_deps_sample": ["judge", "review"],
        }),
    });

    let proposals = (sparse_plan_arbiter::ARBITER.run)(&state);
    assert_eq!(proposals.len(), 1);
    let p = &proposals[0];
    match &p.target {
        InterventionTarget::Agent { agent_id } => assert_eq!(agent_id, "alice"),
        other => panic!("expected Agent target, got {other:?}"),
    }
    match &p.kind {
        InterventionKind::RedispatchWithEscalatedPrompt { prompt } => {
            assert!(prompt.contains("ARBITER"));
            assert!(prompt.contains("judge"));
        }
        other => panic!("expected RedispatchWithEscalatedPrompt, got {other:?}"),
    }
}

#[test]
fn sparse_plan_arbiter_silent_without_help_needed() {
    let state = test_state();
    let proposals = (sparse_plan_arbiter::ARBITER.run)(&state);
    assert!(proposals.is_empty());
}
