use super::*;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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
fn decay_monotonic_lazy() {
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
    let signal = mk_signal("s", SignalKind::HelpNeeded, 0, SignalTarget::Global);
    state.emit_signal(signal);
    // HelpNeeded decay rate is 0.5; after 6 gens, 0.5^6 ~= 0.0156 < 0.05 threshold.
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
    let signal = mk_signal("s", SignalKind::Warning, 0, SignalTarget::Global);
    state.emit_signal(signal);
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
    // Strengths must be monotonically non-increasing.
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
    // Both signals emit at the same effective gen (0) so their effective
    // strengths are identical; tiebreak must favour the newer posted_at_gen.
    state.emit_signal(mk("old", 0));
    state.emit_signal(mk("new", 3));
    // Advance so both have decayed equally relative to posted_at_gen diff.
    // Actually we want same effective strength: set generation == 3, so
    // `old` decays by 3 ticks and `new` by 0. Instead we keep generation at 0
    // (default), which means `new` hasn't been "posted in the future" — its
    // effective strength equals initial. Test the tie at gen=0.
    // With gen=0 both have initial_strength 1.0 (posted in the future for
    // `new` is treated via saturating_sub → 0 delta → 1.0). Tie by posted_at
    // → `new` (3) comes first.
    let sorted = state.signals_sorted_by_strength();
    assert_eq!(sorted[0].0.id, "new");
    assert_eq!(sorted[1].0.id, "old");
}

fn mk_claim(id: &str, kind: ClaimKind, target: ClaimTarget, gen: u64, ttl: u64) -> Claim {
    Claim {
        id: id.into(),
        kind,
        target,
        claimed_by: "agent-a".into(),
        claimed_at_gen: gen,
        ttl_gens: ttl,
        rationale: "test".into(),
    }
}

#[test]
fn claim_compat_exclusive_x_shared_conflict() {
    let path = PathBuf::from("src/lib.rs");
    let a = mk_claim(
        "a",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File { path: path.clone() },
        0,
        5,
    );
    let b = mk_claim("b", ClaimKind::SharedRead, ClaimTarget::File { path }, 0, 5);
    assert!(claims_conflict(&a, &b));
    assert!(claims_conflict(&b, &a));
}

#[test]
fn claim_compat_shared_x_shared_ok() {
    let path = PathBuf::from("src/lib.rs");
    let a = mk_claim(
        "a",
        ClaimKind::SharedRead,
        ClaimTarget::File { path: path.clone() },
        0,
        5,
    );
    let b = mk_claim("b", ClaimKind::SharedRead, ClaimTarget::File { path }, 0, 5);
    assert!(!claims_conflict(&a, &b));
}

#[test]
fn claim_compat_append_x_append_ok() {
    let path = PathBuf::from("src/lib.rs");
    let a = mk_claim(
        "a",
        ClaimKind::AppendOnly,
        ClaimTarget::File { path: path.clone() },
        0,
        5,
    );
    let b = mk_claim("b", ClaimKind::AppendOnly, ClaimTarget::File { path }, 0, 5);
    assert!(!claims_conflict(&a, &b));
}

#[test]
fn claim_compat_soft_never_conflicts() {
    let path = PathBuf::from("src/lib.rs");
    let soft = mk_claim(
        "soft",
        ClaimKind::Soft,
        ClaimTarget::File { path: path.clone() },
        0,
        5,
    );
    let excl = mk_claim(
        "excl",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File { path },
        0,
        5,
    );
    assert!(!claims_conflict(&soft, &excl));
    assert!(!claims_conflict(&excl, &soft));
}

#[test]
fn claim_target_region_overlaps_by_lines() {
    let path_a = PathBuf::from("src/lib.rs");
    let path_b = PathBuf::from("src/other.rs");
    let r1 = ClaimTarget::Region {
        path: path_a.clone(),
        start_line: 10,
        end_line: 20,
    };
    let r2_overlap = ClaimTarget::Region {
        path: path_a.clone(),
        start_line: 15,
        end_line: 25,
    };
    let r3_disjoint = ClaimTarget::Region {
        path: path_a.clone(),
        start_line: 30,
        end_line: 40,
    };
    let r4_other_file = ClaimTarget::Region {
        path: path_b,
        start_line: 15,
        end_line: 25,
    };
    assert!(targets_overlap(&r1, &r2_overlap));
    assert!(!targets_overlap(&r1, &r3_disjoint));
    assert!(!targets_overlap(&r1, &r4_other_file));
}

#[test]
fn claim_target_file_subsumes_region_on_same_path() {
    let path = PathBuf::from("src/lib.rs");
    let file = ClaimTarget::File { path: path.clone() };
    let region = ClaimTarget::Region {
        path,
        start_line: 5,
        end_line: 10,
    };
    assert!(targets_overlap(&file, &region));
    assert!(targets_overlap(&region, &file));
}

#[test]
fn claim_expiry_removes_past_ttl() {
    let mut state = SubstrateState::new();
    let claim = mk_claim(
        "c1",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File {
            path: PathBuf::from("a.rs"),
        },
        0,
        1,
    );
    state.assert_claim(claim).unwrap();
    assert_eq!(state.claims.len(), 1);
    // Advance generation past TTL (0 + 1 = 1, so gen=2 is past-expiry).
    state.advance_generation();
    state.advance_generation();
    let removed = state.expire_claims(state.current_generation());
    assert_eq!(removed, 1);
    assert!(state.claims.is_empty());
}

#[test]
fn tolerant_load_of_phase1_empty_claims() {
    let root = temp_dir("substrate-phase1-claims");
    let dir = root.join(".nit").join("substrate");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{"generation":3,"signals":{},"claims":{},"observations":[]}"#,
    )
    .unwrap();

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.generation, 3);
    assert!(loaded.claims.is_empty());
    assert_eq!(loaded.claim_counter, 0);
}

#[test]
fn assert_claim_inserts_on_no_conflict() {
    let mut state = SubstrateState::new();
    let claim = mk_claim(
        "c1",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File {
            path: PathBuf::from("a.rs"),
        },
        0,
        5,
    );
    state.assert_claim(claim).unwrap();
    assert_eq!(state.claims.len(), 1);
}

#[test]
fn claims_sorted_by_remaining_ttl_empty() {
    let state = SubstrateState::default();
    assert!(state.claims_sorted_by_remaining_ttl().is_empty());
}

#[test]
fn claims_sorted_by_remaining_ttl_filters_expired() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    // Non-expired: 2 + 10 = 12, current 5 → remaining 7.
    let live = mk_claim(
        "live",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File {
            path: PathBuf::from("a.rs"),
        },
        2,
        10,
    );
    // Expired: 0 + 3 = 3, current 5 → past TTL.
    let expired = mk_claim(
        "expired",
        ClaimKind::SharedRead,
        ClaimTarget::File {
            path: PathBuf::from("b.rs"),
        },
        0,
        3,
    );
    state.claims.insert(live.id.clone(), live);
    state.claims.insert(expired.id.clone(), expired);
    let sorted = state.claims_sorted_by_remaining_ttl();
    assert_eq!(sorted.len(), 1);
    assert_eq!(sorted[0].0.id, "live");
    assert_eq!(sorted[0].1, 7);
}

#[test]
fn claims_sorted_by_remaining_ttl_descending() {
    let mut state = SubstrateState::new();
    state.generation = 0;
    // Three claims, same claimed_at_gen (0), different TTLs.
    let short = mk_claim(
        "short",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File {
            path: PathBuf::from("a.rs"),
        },
        0,
        1,
    );
    let mid = mk_claim(
        "mid",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File {
            path: PathBuf::from("b.rs"),
        },
        0,
        5,
    );
    let long = mk_claim(
        "long",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File {
            path: PathBuf::from("c.rs"),
        },
        0,
        10,
    );
    state.claims.insert(short.id.clone(), short);
    state.claims.insert(mid.id.clone(), mid);
    state.claims.insert(long.id.clone(), long);
    let sorted = state.claims_sorted_by_remaining_ttl();
    let ids: Vec<&str> = sorted.iter().map(|(c, _)| c.id.as_str()).collect();
    assert_eq!(ids, vec!["long", "mid", "short"]);
    // Remaining must be monotonically non-increasing.
    for pair in sorted.windows(2) {
        assert!(pair[0].1 >= pair[1].1);
    }
}

#[test]
fn claims_sorted_by_remaining_ttl_tiebreak_by_claimed_at_gen() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    // Both remaining = 10, but newer claimed_at_gen wins on tiebreak.
    // old: claimed_at_gen=0, ttl=15 → expiry 15, remaining 10.
    // new: claimed_at_gen=3, ttl=12 → expiry 15, remaining 10.
    let old = mk_claim(
        "old",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File {
            path: PathBuf::from("a.rs"),
        },
        0,
        15,
    );
    let new = mk_claim(
        "new",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File {
            path: PathBuf::from("b.rs"),
        },
        3,
        12,
    );
    state.claims.insert(old.id.clone(), old);
    state.claims.insert(new.id.clone(), new);
    let sorted = state.claims_sorted_by_remaining_ttl();
    assert_eq!(sorted.len(), 2);
    assert_eq!(sorted[0].0.id, "new");
    assert_eq!(sorted[1].0.id, "old");
    assert_eq!(sorted[0].1, sorted[1].1);
}

#[test]
fn assert_claim_returns_err_on_conflict() {
    let mut state = SubstrateState::new();
    let path = PathBuf::from("a.rs");
    let first = mk_claim(
        "c1",
        ClaimKind::ExclusiveWrite,
        ClaimTarget::File { path: path.clone() },
        0,
        5,
    );
    state.assert_claim(first).unwrap();

    let second = Claim {
        id: "c2".into(),
        kind: ClaimKind::ExclusiveWrite,
        target: ClaimTarget::File { path },
        claimed_by: "agent-b".into(),
        claimed_at_gen: 0,
        ttl_gens: 5,
        rationale: "conflicting".into(),
    };
    let err = state.assert_claim(second).expect_err("should conflict");
    assert_eq!(err.conflicts.len(), 1);
    assert_eq!(err.conflicts[0].id, "c1");
    // Original claim must still be in the map; second must NOT have been inserted.
    assert_eq!(state.claims.len(), 1);
}

fn mk_assumption(
    id: &str,
    target: AssumptionTarget,
    gen: u64,
    ttl: u64,
    fact: serde_json::Value,
) -> Assumption {
    Assumption {
        id: id.into(),
        target,
        fact,
        posted_by: "agent-a".into(),
        posted_at_gen: gen,
        ttl_gens: ttl,
        rationale: "test-assumption".into(),
    }
}

#[test]
fn default_state_has_empty_assumptions() {
    let state = SubstrateState::default();
    assert!(state.assumptions.is_empty());
    assert_eq!(state.assumption_counter, 0);
}

#[test]
fn assumption_round_trip_serialization() {
    let cases = vec![
        mk_assumption(
            "a-file",
            AssumptionTarget::File {
                path: PathBuf::from("src/lib.rs"),
            },
            1,
            5,
            serde_json::json!({"fact": "file-level"}),
        ),
        mk_assumption(
            "a-region",
            AssumptionTarget::Region {
                path: PathBuf::from("src/lib.rs"),
                start_line: 10,
                end_line: 20,
            },
            2,
            8,
            serde_json::json!({"fact": "region-level"}),
        ),
        mk_assumption(
            "a-global",
            AssumptionTarget::Global,
            3,
            11,
            serde_json::json!({"fact": "global"}),
        ),
    ];

    for original in cases {
        let json = serde_json::to_string(&original).unwrap();
        let restored: Assumption = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.target, original.target);
        assert_eq!(restored.fact, original.fact);
        assert_eq!(restored.posted_by, original.posted_by);
        assert_eq!(restored.posted_at_gen, original.posted_at_gen);
        assert_eq!(restored.ttl_gens, original.ttl_gens);
        assert_eq!(restored.rationale, original.rationale);
    }
}

#[test]
fn assumption_targets_overlap_matches_claims_geometry() {
    use crate::substrate::assumption_targets_overlap;
    let path_a = PathBuf::from("a.rs");
    let path_b = PathBuf::from("b.rs");

    // File+File same path
    assert!(assumption_targets_overlap(
        &AssumptionTarget::File {
            path: path_a.clone()
        },
        &AssumptionTarget::File {
            path: path_a.clone()
        },
    ));
    // File+File different paths
    assert!(!assumption_targets_overlap(
        &AssumptionTarget::File {
            path: path_a.clone()
        },
        &AssumptionTarget::File {
            path: path_b.clone()
        },
    ));
    // Region+Region overlapping lines
    assert!(assumption_targets_overlap(
        &AssumptionTarget::Region {
            path: path_a.clone(),
            start_line: 10,
            end_line: 20,
        },
        &AssumptionTarget::Region {
            path: path_a.clone(),
            start_line: 15,
            end_line: 25,
        },
    ));
    // Non-overlapping Region same path
    assert!(!assumption_targets_overlap(
        &AssumptionTarget::Region {
            path: path_a.clone(),
            start_line: 10,
            end_line: 20,
        },
        &AssumptionTarget::Region {
            path: path_a.clone(),
            start_line: 30,
            end_line: 40,
        },
    ));
    // Global + anything
    assert!(assumption_targets_overlap(
        &AssumptionTarget::Global,
        &AssumptionTarget::File {
            path: path_a.clone()
        },
    ));
    assert!(assumption_targets_overlap(
        &AssumptionTarget::File { path: path_a },
        &AssumptionTarget::Global,
    ));
    assert!(assumption_targets_overlap(
        &AssumptionTarget::Global,
        &AssumptionTarget::Global,
    ));
}

#[test]
fn assert_assumption_inserts_and_is_idempotent_on_same_id() {
    let mut state = SubstrateState::new();
    let a = mk_assumption(
        "dup",
        AssumptionTarget::File {
            path: PathBuf::from("src/a.rs"),
        },
        0,
        5,
        serde_json::json!({"v": 1}),
    );
    state.assert_assumption(a);
    assert_eq!(state.assumptions.len(), 1);

    // Insert again with same id — should remain size 1 (idempotent by id).
    let a2 = mk_assumption(
        "dup",
        AssumptionTarget::File {
            path: PathBuf::from("src/a.rs"),
        },
        0,
        5,
        serde_json::json!({"v": 2}),
    );
    state.assert_assumption(a2);
    assert_eq!(state.assumptions.len(), 1);
}

#[test]
fn assumption_expiry_removes_past_ttl() {
    let mut state = SubstrateState::new();
    let a = mk_assumption(
        "e1",
        AssumptionTarget::File {
            path: PathBuf::from("a.rs"),
        },
        0,
        2,
        serde_json::json!({}),
    );
    state.assert_assumption(a);
    assert_eq!(state.assumptions.len(), 1);

    state.generation = 5;
    let removed = state.expire_assumptions(5);
    assert_eq!(removed, 1);
    assert!(state.assumptions.is_empty());
}

#[test]
fn tolerant_load_of_phase3_missing_assumptions_field() {
    let root = temp_dir("substrate-phase3-no-assumptions");
    let dir = root.join(".nit").join("substrate");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{"generation":3,"signals":{},"claims":{},"observations":[]}"#,
    )
    .unwrap();

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.generation, 3);
    assert!(loaded.assumptions.is_empty());
    assert_eq!(loaded.assumption_counter, 0);
}

#[test]
fn next_assumption_id_format_and_monotonic() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    let first = state.next_assumption_id("a1");
    let second = state.next_assumption_id("a1");
    assert_eq!(first, "5-a1-0");
    assert_eq!(second, "5-a1-1");
    assert_eq!(state.assumption_counter, 2);
}

#[test]
fn assumptions_sorted_by_remaining_ttl_empty() {
    let state = SubstrateState::default();
    assert!(state.assumptions_sorted_by_remaining_ttl().is_empty());
}

#[test]
fn assumptions_sorted_by_remaining_ttl_descending_with_tiebreak() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    // Three assumptions, varying remaining TTL.
    //  long   : posted_at=0, ttl=20 → expiry 20, remaining 15
    //  mid_a  : posted_at=1, ttl=12 → expiry 13, remaining 8
    //  mid_b  : posted_at=3, ttl=10 → expiry 13, remaining 8 (tiebreak — newer wins)
    //  short  : posted_at=2, ttl=5  → expiry 7,  remaining 2
    let long = mk_assumption(
        "long",
        AssumptionTarget::File {
            path: PathBuf::from("a.rs"),
        },
        0,
        20,
        serde_json::json!({}),
    );
    let mid_a = mk_assumption(
        "mid-a",
        AssumptionTarget::File {
            path: PathBuf::from("b.rs"),
        },
        1,
        12,
        serde_json::json!({}),
    );
    let mid_b = mk_assumption(
        "mid-b",
        AssumptionTarget::File {
            path: PathBuf::from("c.rs"),
        },
        3,
        10,
        serde_json::json!({}),
    );
    let short = mk_assumption(
        "short",
        AssumptionTarget::File {
            path: PathBuf::from("d.rs"),
        },
        2,
        5,
        serde_json::json!({}),
    );
    state.assert_assumption(long);
    state.assert_assumption(mid_a);
    state.assert_assumption(mid_b);
    state.assert_assumption(short);
    let sorted = state.assumptions_sorted_by_remaining_ttl();
    let ids: Vec<&str> = sorted.iter().map(|(a, _)| a.id.as_str()).collect();
    // long first, then mid_b (newer posted_at wins on tiebreak), mid_a, short.
    assert_eq!(ids, vec!["long", "mid-b", "mid-a", "short"]);
    // Remaining column must be monotonically non-increasing.
    for pair in sorted.windows(2) {
        assert!(pair[0].1 >= pair[1].1);
    }
}

#[test]
fn assumptions_sorted_by_remaining_ttl_filters_expired() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    // Live: posted_at=2, ttl=10 → expiry 12, remaining 7.
    let live = mk_assumption(
        "live",
        AssumptionTarget::File {
            path: PathBuf::from("a.rs"),
        },
        2,
        10,
        serde_json::json!({}),
    );
    // Expired: posted_at=0, ttl=3 → expiry 3, current 5 → past TTL.
    let expired = mk_assumption(
        "expired",
        AssumptionTarget::File {
            path: PathBuf::from("b.rs"),
        },
        0,
        3,
        serde_json::json!({}),
    );
    state.assert_assumption(live);
    state.assert_assumption(expired);
    let sorted = state.assumptions_sorted_by_remaining_ttl();
    assert_eq!(sorted.len(), 1);
    assert_eq!(sorted[0].0.id, "live");
    assert_eq!(sorted[0].1, 7);
}
