//! Tests for `metabolism::tick` — per-tick decay, pruning, and the
//! observer→arbiter cascade with mood modulation.

use std::fs;
use std::path::PathBuf;

use crate::metabolism::tick;
use crate::state::AppState;
use crate::substrate::{
    Assumption, AssumptionTarget, Claim, ClaimKind, ClaimTarget, Signal, SignalKind, SignalTarget,
};
use crate::test_helpers::{temp_dir, test_state_in};

fn fixture_state(label: &str) -> AppState {
    test_state_in(temp_dir(label))
}

fn seed_claim(state: &mut AppState, id: &str, path: &str, ttl_gens: u64) {
    state.substrate.claims.insert(
        id.to_string(),
        Claim {
            id: id.to_string(),
            kind: ClaimKind::ExclusiveWrite,
            target: ClaimTarget::File {
                path: PathBuf::from(path),
            },
            claimed_by: "seed".to_string(),
            claimed_at_gen: 0,
            ttl_gens,
            rationale: "test".to_string(),
        },
    );
}

fn seed_warning(state: &mut AppState, id: &str, posted_by: &str, posted_at_gen: u64) {
    state.substrate.signals.insert(
        id.to_string(),
        Signal {
            id: id.to_string(),
            kind: SignalKind::Warning,
            posted_by: posted_by.to_string(),
            posted_at_gen,
            target: SignalTarget::Global,
            initial_strength: 1.0,
            payload: serde_json::Value::Null,
        },
    );
}

fn count_help_needed(state: &AppState, posted_by: &str) -> usize {
    state
        .substrate
        .signals
        .values()
        .filter(|s| s.kind == SignalKind::HelpNeeded && s.posted_by == posted_by)
        .count()
}

#[test]
fn tick_expires_claims_past_ttl() {
    let mut state = fixture_state("metabolism-expire-claims");
    state.substrate.generation = 5;
    seed_claim(&mut state, "c-seed-0", "foo.rs", 2);

    let outcome = tick(&mut state);

    assert!(state.substrate.claims.is_empty());
    assert_eq!(outcome.claims_expired, 1);
}

#[test]
fn tick_prunes_decayed_signals() {
    let mut state = fixture_state("metabolism-prune");
    // Posted at gen 0; fast-forward past the decay floor.
    state.substrate.generation = 50;
    seed_warning(&mut state, "s-seed-0", "seed", 0);

    let outcome = tick(&mut state);

    assert!(state.substrate.signals.is_empty());
    assert_eq!(outcome.signals_pruned, 1);
}

#[test]
fn tick_does_not_advance_generation() {
    let mut state = fixture_state("metabolism-no-advance");
    state.substrate.generation = 7;
    let _ = tick(&mut state);
    assert_eq!(state.substrate.current_generation(), 7);
}

#[test]
fn tick_runs_observers() {
    let mut state = fixture_state("metabolism-observers");
    // Two warnings from the same agent — repeat_failure should escalate.
    let gen = state.substrate.generation;
    for i in 0..2 {
        seed_warning(&mut state, &format!("w-{i}"), "a1", gen);
    }

    let outcome = tick(&mut state);

    assert!(outcome.observer_emissions >= 1);
    assert!(count_help_needed(&state, "observer:repeat_failure") >= 1);
}

#[test]
fn tick_is_noop_when_idle() {
    let mut state = fixture_state("metabolism-noop");
    let outcome = tick(&mut state);
    assert!(outcome.is_noop(), "expected noop, got {outcome:?}");
    assert!(!outcome.saved);
}

#[test]
fn tick_saves_only_when_dirty() {
    let mut state = fixture_state("metabolism-dirty-save");
    let state_file = state
        .workspace_root
        .join(".nit")
        .join("substrate")
        .join("state.json");

    state.substrate.generation = 5;
    seed_claim(&mut state, "c-seed-0", "foo.rs", 2);

    let first = tick(&mut state);
    assert!(first.saved);
    assert!(state_file.exists());
    let contents_after_first = fs::read(&state_file).unwrap();

    // Second tick is a noop — content must not change.
    let second = tick(&mut state);
    assert!(second.is_noop());
    assert!(!second.saved);
    let contents_after_second = fs::read(&state_file).unwrap();
    assert_eq!(contents_after_first, contents_after_second);
}

#[test]
fn tick_expires_assumptions_past_ttl() {
    let mut state = fixture_state("metabolism-expire-assumptions");
    state.substrate.generation = 5;
    let assumption = Assumption {
        id: "a-seed-0".to_string(),
        target: AssumptionTarget::File {
            path: PathBuf::from("foo.rs"),
        },
        fact: serde_json::json!({}),
        posted_by: "seed".to_string(),
        posted_at_gen: 0,
        ttl_gens: 2,
        rationale: "test".to_string(),
    };
    state
        .substrate
        .assumptions
        .insert(assumption.id.clone(), assumption);

    let outcome = tick(&mut state);

    assert!(state.substrate.assumptions.is_empty());
    assert_eq!(outcome.assumptions_expired, 1);
}

#[test]
fn consecutive_metabolic_ticks_preserve_observer_cooldown() {
    let mut state = fixture_state("metabolism-cooldown");
    let gen = state.substrate.generation;
    for i in 0..2 {
        seed_warning(&mut state, &format!("w-{i}"), "a1", gen);
    }

    let first = tick(&mut state);
    assert!(first.observer_emissions >= 1);

    let second = tick(&mut state);
    // Self-silencing: no new HelpNeeded in the second tick.
    assert_eq!(second.observer_emissions, 0);

    assert_eq!(
        count_help_needed(&state, "observer:repeat_failure"),
        1,
        "only one HelpNeeded should exist",
    );
}
