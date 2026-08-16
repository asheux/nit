//! Substrate tests grouped by concern. The mounted entry; submodules
//! cover the per-primitive geometries (signals here, claims and
//! assumptions in their own files). Local fixtures stay co-located with
//! the assertions that depend on them.

use super::*;
use crate::mood::Mood;
use crate::test_helpers::temp_dir;
use std::fs;
use std::path::PathBuf;

#[path = "substrate/assumptions.rs"]
mod assumptions;
#[path = "substrate/claims.rs"]
mod claims;

fn mk_signal(id: &str, kind: SignalKind, posted_at_gen: u64, target: SignalTarget) -> Signal {
    Signal {
        id: id.into(),
        kind,
        posted_by: "agent-a".into(),
        posted_at_gen,
        target,
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::Value::Null,
    }
}

#[test]
fn default_state_has_zero_generation() {
    let state = SubstrateState::default();
    assert_eq!(state.current_generation(), 0);
    assert!(state.signals.is_empty());
    assert!(state.claims.is_empty());
    assert!(state.observations.is_empty());
}

#[test]
fn advance_generation_is_monotonic() {
    let mut state = SubstrateState::new();
    assert_eq!(state.advance_generation(), 1);
    assert_eq!(state.advance_generation(), 2);
    assert_eq!(state.advance_generation(), 3);
    assert_eq!(state.current_generation(), 3);
}

#[test]
fn round_trip_serialization() {
    let mut state = SubstrateState::new();
    state.advance_generation();
    state.advance_generation();
    let json = serde_json::to_string(&state).unwrap();
    let restored: SubstrateState = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.generation, 2);
}

#[test]
fn save_then_load_round_trip() {
    let root = temp_dir("substrate-roundtrip");
    let state = SubstrateState::new();
    state.save(&root).unwrap();

    let expected_path = root.join(".nit").join("substrate").join("state.json");
    assert!(expected_path.exists(), "state file should exist after save");

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.generation, state.generation);

    let mut next = loaded;
    next.advance_generation();
    next.save(&root).unwrap();
    let reloaded = SubstrateState::load(&root);
    assert_eq!(reloaded.generation, 1);
}

#[test]
fn load_from_missing_dir_yields_default() {
    let root = temp_dir("substrate-missing");
    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.current_generation(), 0);
    assert!(loaded.signals.is_empty());
    assert!(loaded.claims.is_empty());
    assert!(loaded.observations.is_empty());
}

#[test]
fn load_from_corrupt_file_yields_default() {
    let root = temp_dir("substrate-corrupt");
    let dir = root.join(".nit").join("substrate");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("state.json"), "not json {{").unwrap();

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.current_generation(), 0);
    assert!(loaded.signals.is_empty());
    assert!(loaded.claims.is_empty());
    assert!(loaded.observations.is_empty());
}

#[test]
fn signal_round_trip_serialization() {
    let cases = vec![
        mk_signal(
            "s-warn",
            SignalKind::Warning,
            1,
            SignalTarget::File {
                path: PathBuf::from("src/lib.rs"),
            },
        ),
        mk_signal(
            "s-help",
            SignalKind::HelpNeeded,
            2,
            SignalTarget::Agent {
                agent_id: "agent-b".into(),
            },
        ),
        mk_signal("s-done", SignalKind::DoneMarker, 3, SignalTarget::Global),
    ];

    for original in cases {
        let json = serde_json::to_string(&original).unwrap();
        let restored: Signal = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.kind, original.kind);
        assert_eq!(restored.posted_by, original.posted_by);
        assert_eq!(restored.posted_at_gen, original.posted_at_gen);
        assert_eq!(restored.target, original.target);
        assert_eq!(restored.initial_strength, original.initial_strength);
        assert_eq!(restored.payload, original.payload);
    }
}

#[test]
fn decay_is_monotonic_and_lazy() {
    let signal = mk_signal("s", SignalKind::DoneMarker, 0, SignalTarget::Global);
    let s0 = signal.effective_strength(0);
    let s1 = signal.effective_strength(1);
    let s2 = signal.effective_strength(2);
    let s5 = signal.effective_strength(5);
    assert_eq!(s0, 1.0);
    assert!(s1 < s0);
    assert!(s2 < s1);
    assert!(s5 < s2);
}

#[test]
fn decay_rate_varies_by_kind() {
    let done = mk_signal("d", SignalKind::DoneMarker, 0, SignalTarget::Global);
    let help = mk_signal("h", SignalKind::HelpNeeded, 0, SignalTarget::Global);
    assert!(done.effective_strength(3) > help.effective_strength(3));
}

#[test]
fn prune_removes_below_threshold() {
    let mut state = SubstrateState::new();
    state.emit_signal(mk_signal(
        "s",
        SignalKind::HelpNeeded,
        0,
        SignalTarget::Global,
    ));
    // HelpNeeded decay rate is 0.5; 0.5^6 ≈ 0.0156 < 0.05 prune threshold.
    for _ in 0..6 {
        state.advance_generation();
    }
    let removed = state.prune_signals_below(SubstrateState::DEFAULT_PRUNE_THRESHOLD);
    assert!(removed >= 1);
    assert!(state.signals.is_empty());
}

#[test]
fn signals_iter_yields_effective_strength() {
    let mut state = SubstrateState::new();
    state.emit_signal(mk_signal("s", SignalKind::Warning, 0, SignalTarget::Global));
    state.advance_generation();
    state.advance_generation();
    let (found, strength) = state.signals_iter().next().unwrap();
    assert_eq!(
        strength,
        found.effective_strength(state.current_generation())
    );
}

#[test]
fn signals_by_kind_filter() {
    let mut state = SubstrateState::new();
    state.emit_signal(mk_signal("w", SignalKind::Warning, 0, SignalTarget::Global));
    state.emit_signal(mk_signal("l", SignalKind::Lead, 0, SignalTarget::Global));
    let warnings: Vec<_> = state.signals_by_kind(SignalKind::Warning).collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].0.kind, SignalKind::Warning);
}

#[test]
fn signals_by_target_file_filter() {
    let mut state = SubstrateState::new();
    let target_a = SignalTarget::File {
        path: PathBuf::from("a.rs"),
    };
    let target_b = SignalTarget::File {
        path: PathBuf::from("b.rs"),
    };
    state.emit_signal(mk_signal("a", SignalKind::Lead, 0, target_a.clone()));
    state.emit_signal(mk_signal("b", SignalKind::Lead, 0, target_b));
    let matched: Vec<_> = state.signals_by_target(&target_a).collect();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].0.id, "a");
}

#[test]
fn tolerant_load_of_phase1_empty_signals() {
    let root = temp_dir("substrate-phase1");
    let dir = root.join(".nit").join("substrate");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{"generation":3,"signals":{},"claims":{},"observations":[]}"#,
    )
    .unwrap();

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.generation, 3);
    assert!(loaded.signals.is_empty());
    assert_eq!(loaded.signal_counter, 0);
}

#[test]
fn next_signal_id_format_and_monotonic() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    let first = state.next_signal_id("agent-a");
    let second = state.next_signal_id("agent-a");
    assert_eq!(first, "5-agent-a-0");
    assert_eq!(second, "5-agent-a-1");
    assert_eq!(state.signal_counter, 2);
}

#[test]
fn sorted_by_strength_empty() {
    let state = SubstrateState::default();
    assert!(state.signals_sorted_by_strength().is_empty());
}

#[test]
fn sorted_by_strength_descending() {
    let mut state = SubstrateState::new();
    let mk = |id: &str, strength: f32| Signal {
        id: id.into(),
        kind: SignalKind::Warning,
        posted_by: "agent-a".into(),
        posted_at_gen: 0,
        target: SignalTarget::Global,
        initial_strength: strength,
        payload: serde_json::Value::Null,
    };
    state.emit_signal(mk("mid", 0.5));
    state.emit_signal(mk("high", 0.9));
    state.emit_signal(mk("low", 0.1));

    let sorted = state.signals_sorted_by_strength();
    let ids: Vec<&str> = sorted.iter().map(|(s, _)| s.id.as_str()).collect();
    assert_eq!(ids, vec!["high", "mid", "low"]);
    for pair in sorted.windows(2) {
        assert!(pair[0].1 >= pair[1].1);
    }
}

#[test]
fn sorted_by_strength_tiebreak_by_posted_gen() {
    let mut state = SubstrateState::new();
    let mk = |id: &str, posted: u64| Signal {
        id: id.into(),
        kind: SignalKind::DoneMarker,
        posted_by: "agent-a".into(),
        posted_at_gen: posted,
        target: SignalTarget::Global,
        initial_strength: 1.0,
        payload: serde_json::Value::Null,
    };
    // Both signals have the same effective strength at gen=0 (initial 1.0,
    // no decay applied — `posted_at_gen` "in the future" saturates to 0
    // delta). The tiebreak prefers the newer posted_at_gen.
    state.emit_signal(mk("old", 0));
    state.emit_signal(mk("new", 3));

    let sorted = state.signals_sorted_by_strength();
    assert_eq!(sorted[0].0.id, "new");
    assert_eq!(sorted[1].0.id, "old");
}

fn excl_claim(id: &str, owner: &str, path: &str) -> Claim {
    Claim {
        id: id.into(),
        kind: ClaimKind::ExclusiveWrite,
        target: ClaimTarget::File {
            path: PathBuf::from(path),
        },
        claimed_by: owner.into(),
        claimed_at_gen: 0,
        ttl_gens: 50,
        rationale: "test".into(),
    }
}

#[test]
fn reconcile_unions_disjoint_primitives() {
    let mut a = SubstrateState::new();
    a.emit_signal(mk_signal(
        "a-sig",
        SignalKind::Warning,
        0,
        SignalTarget::Global,
    ));
    a.claims
        .insert("a-claim".into(), excl_claim("a-claim", "agent-a", "a.rs"));

    let mut b = SubstrateState::new();
    b.emit_signal(mk_signal(
        "b-sig",
        SignalKind::Lead,
        0,
        SignalTarget::Global,
    ));
    b.assert_assumption(Assumption {
        id: "b-assume".into(),
        target: AssumptionTarget::Global,
        fact: serde_json::Value::Bool(true),
        posted_by: "agent-b".into(),
        posted_at_gen: 0,
        ttl_gens: 50,
        rationale: "test".into(),
    });

    let merged = SubstrateState::reconcile(&a, &b);
    assert!(merged.signals.contains_key("a-sig"));
    assert!(merged.signals.contains_key("b-sig"));
    assert!(merged.claims.contains_key("a-claim"));
    assert!(merged.assumptions.contains_key("b-assume"));
}

#[test]
fn reconcile_retains_conflicting_claims_for_the_oracle() {
    // Two branches each take an ExclusiveWrite lock on the SAME file under
    // different owners — `claims_conflict` would reject the second in a single
    // timeline. reconcile keeps BOTH: it is a provenance union, and the real
    // conflict is the divergent code, resolved later by the merge oracle.
    let ca = excl_claim("a-lock", "agent-a", "shared.rs");
    let cb = excl_claim("b-lock", "agent-b", "shared.rs");
    assert!(
        claims_conflict(&ca, &cb),
        "precondition: the claims contend"
    );

    let mut a = SubstrateState::new();
    a.claims.insert(ca.id.clone(), ca);
    let mut b = SubstrateState::new();
    b.claims.insert(cb.id.clone(), cb);

    let merged = SubstrateState::reconcile(&a, &b);
    assert_eq!(merged.claims.len(), 2);
    assert!(merged.claims.contains_key("a-lock"));
    assert!(merged.claims.contains_key("b-lock"));
}

#[test]
fn reconcile_takes_max_generation_and_counters() {
    let mut a = SubstrateState::new();
    a.generation = 3;
    a.signal_counter = 9;
    a.claim_counter = 1;
    a.assumption_counter = 4;

    let mut b = SubstrateState::new();
    b.generation = 7;
    b.signal_counter = 2;
    b.claim_counter = 5;
    b.assumption_counter = 4;

    let merged = SubstrateState::reconcile(&a, &b);
    assert_eq!(merged.generation, 7);
    // Per-field max keeps a post-merge mint from colliding with either parent.
    assert_eq!(merged.signal_counter, 9);
    assert_eq!(merged.claim_counter, 5);
    assert_eq!(merged.assumption_counter, 4);
}

#[test]
fn reconcile_mood_follows_the_higher_generation_branch() {
    let mut a = SubstrateState::new();
    a.generation = 2;
    a.mood = Mood::Exploration;
    a.mood_quiet_streak = 1;

    let mut b = SubstrateState::new();
    b.generation = 5;
    b.mood = Mood::Defensive;
    b.mood_quiet_streak = 8;

    let merged = SubstrateState::reconcile(&a, &b);
    assert_eq!(merged.mood, Mood::Defensive);
    assert_eq!(merged.mood_quiet_streak, 8);

    // A generation tie keeps a's mood (the branch the merge resolves into).
    let mut tie_a = a.clone();
    tie_a.generation = 5;
    let tie = SubstrateState::reconcile(&tie_a, &b);
    assert_eq!(tie.mood, Mood::Exploration);
}

#[test]
fn reconcile_concatenates_observations_a_then_b() {
    let mut a = SubstrateState::new();
    a.observations.push(serde_json::json!({"from": "a"}));
    let mut b = SubstrateState::new();
    b.observations.push(serde_json::json!({"from": "b"}));

    let merged = SubstrateState::reconcile(&a, &b);
    assert_eq!(merged.observations.len(), 2);
    assert_eq!(merged.observations[0]["from"], "a");
    assert_eq!(merged.observations[1]["from"], "b");
}

#[test]
fn reconcile_b_wins_on_a_shared_id_key() {
    // Maps key on id, so `extend(b over a)` resolves a shared key to b. v1
    // never mints per-branch substrate so this is dormant, but the a-bias is
    // pinned here against a v2 that does.
    let mut a = SubstrateState::new();
    a.claims
        .insert("dup".into(), excl_claim("dup", "agent-a", "a.rs"));
    let mut b = SubstrateState::new();
    b.claims
        .insert("dup".into(), excl_claim("dup", "agent-b", "b.rs"));

    let merged = SubstrateState::reconcile(&a, &b);
    assert_eq!(merged.claims.len(), 1);
    assert_eq!(merged.claims["dup"].claimed_by, "agent-b");
}
