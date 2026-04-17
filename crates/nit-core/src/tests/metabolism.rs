use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::metabolism::tick;
use crate::state::AppState;
use crate::substrate::{
    Assumption, AssumptionTarget, Claim, ClaimKind, ClaimTarget, Signal, SignalKind, SignalTarget,
};
use crate::Buffer;

fn temp_dir(label: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("nit-test-{label}-{now}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_state(label: &str) -> AppState {
    let dir = temp_dir(label);
    let editor = Buffer::from_str("editor", "", None);
    let notes = Buffer::from_str("notes", "", None);
    AppState::new(dir, editor, notes)
}

#[test]
fn tick_expires_claims_past_ttl() {
    let mut state = test_state("metabolism-expire-claims");
    // Claim posted at gen 0 with ttl 2 — expired by gen 5.
    state.substrate.generation = 5;
    let claim = Claim {
        id: "c-seed-0".to_string(),
        kind: ClaimKind::ExclusiveWrite,
        target: ClaimTarget::File { path: PathBuf::from("foo.rs") },
        claimed_by: "seed".to_string(),
        claimed_at_gen: 0,
        ttl_gens: 2,
        rationale: "test".to_string(),
    };
    state.substrate.claims.insert(claim.id.clone(), claim);

    let outcome = tick(&mut state);

    assert!(state.substrate.claims.is_empty());
    assert_eq!(outcome.claims_expired, 1);
}

#[test]
fn tick_prunes_decayed_signals() {
    let mut state = test_state("metabolism-prune");
    // Signal posted at gen 0; fast-forward generation past decay floor.
    state.substrate.generation = 50;
    let signal = Signal {
        id: "s-seed-0".to_string(),
        kind: SignalKind::Warning,
        posted_by: "seed".to_string(),
        posted_at_gen: 0,
        target: SignalTarget::Global,
        initial_strength: 1.0,
        payload: serde_json::Value::Null,
    };
    state.substrate.signals.insert(signal.id.clone(), signal);

    let outcome = tick(&mut state);

    assert!(state.substrate.signals.is_empty());
    assert_eq!(outcome.signals_pruned, 1);
}

#[test]
fn tick_does_not_advance_generation() {
    let mut state = test_state("metabolism-no-advance");
    state.substrate.generation = 7;
    let _ = tick(&mut state);
    assert_eq!(state.substrate.current_generation(), 7);
}

#[test]
fn tick_runs_observers() {
    let mut state = test_state("metabolism-observers");
    // Inject two Warnings posted_by = "a1" so repeat_failure will emit
    // a HelpNeeded.
    for i in 0..2 {
        let s = Signal {
            id: format!("w-{i}"),
            kind: SignalKind::Warning,
            posted_by: "a1".to_string(),
            posted_at_gen: state.substrate.generation,
            target: SignalTarget::Global,
            initial_strength: 1.0,
            payload: serde_json::Value::Null,
        };
        state.substrate.signals.insert(s.id.clone(), s);
    }

    let outcome = tick(&mut state);

    assert!(outcome.observer_emissions >= 1);
    let help_needed = state
        .substrate
        .signals
        .values()
        .any(|s| s.posted_by == "observer:repeat_failure" && s.kind == SignalKind::HelpNeeded);
    assert!(help_needed);
}

#[test]
fn tick_is_noop_when_idle() {
    let mut state = test_state("metabolism-noop");
    let outcome = tick(&mut state);
    assert!(outcome.is_noop(), "expected noop, got {:?}", outcome);
    assert!(!outcome.saved);
}

#[test]
fn tick_saves_only_when_dirty() {
    let mut state = test_state("metabolism-dirty-save");
    let state_file = state
        .workspace_root
        .join(".nit")
        .join("substrate")
        .join("state.json");

    // Force a dirty tick: seed an expired claim.
    state.substrate.generation = 5;
    let claim = Claim {
        id: "c-seed-0".to_string(),
        kind: ClaimKind::ExclusiveWrite,
        target: ClaimTarget::File { path: PathBuf::from("foo.rs") },
        claimed_by: "seed".to_string(),
        claimed_at_gen: 0,
        ttl_gens: 2,
        rationale: "test".to_string(),
    };
    state.substrate.claims.insert(claim.id.clone(), claim);

    let first = tick(&mut state);
    assert!(first.saved);
    assert!(state_file.exists());
    let contents_after_first = fs::read(&state_file).unwrap();

    // Second tick is noop — content must not change.
    let second = tick(&mut state);
    assert!(second.is_noop());
    assert!(!second.saved);
    let contents_after_second = fs::read(&state_file).unwrap();
    assert_eq!(contents_after_first, contents_after_second);
}

#[test]
fn tick_expires_assumptions_past_ttl() {
    let mut state = test_state("metabolism-expire-assumptions");
    // Assumption posted at gen 0 with ttl 2 — expired by gen 5.
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
    let mut state = test_state("metabolism-cooldown");
    // Inject warnings so repeat_failure emits HelpNeeded on first tick.
    for i in 0..2 {
        let s = Signal {
            id: format!("w-{i}"),
            kind: SignalKind::Warning,
            posted_by: "a1".to_string(),
            posted_at_gen: state.substrate.generation,
            target: SignalTarget::Global,
            initial_strength: 1.0,
            payload: serde_json::Value::Null,
        };
        state.substrate.signals.insert(s.id.clone(), s);
    }

    let first = tick(&mut state);
    assert!(first.observer_emissions >= 1);

    let second = tick(&mut state);
    // Self-silencing: no new HelpNeeded in the second tick.
    assert_eq!(second.observer_emissions, 0);

    let help_count = state
        .substrate
        .signals
        .values()
        .filter(|s| s.posted_by == "observer:repeat_failure" && s.kind == SignalKind::HelpNeeded)
        .count();
    assert_eq!(help_count, 1, "only one HelpNeeded should exist");
}
