//! Assumption fact storage: serialization, geometry overlap with claim
//! targets, expiry, and the TTL-sorted view consumed by the substrate overlay.

use super::*;

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

fn mk_file_assumption(id: &str, path: &str, gen: u64, ttl: u64) -> Assumption {
    mk_assumption(
        id,
        AssumptionTarget::File {
            path: PathBuf::from(path),
        },
        gen,
        ttl,
        serde_json::json!({}),
    )
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

    // File+File same path → overlap.
    assert!(assumption_targets_overlap(
        &AssumptionTarget::File {
            path: path_a.clone()
        },
        &AssumptionTarget::File {
            path: path_a.clone()
        },
    ));
    // File+File different paths → disjoint.
    assert!(!assumption_targets_overlap(
        &AssumptionTarget::File {
            path: path_a.clone()
        },
        &AssumptionTarget::File {
            path: path_b.clone()
        },
    ));
    // Region+Region overlapping lines → overlap.
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
    // Region+Region same path, disjoint lines → no overlap.
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
    // Global overlaps everything (and itself).
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
    state.assert_assumption(mk_assumption(
        "dup",
        AssumptionTarget::File {
            path: PathBuf::from("src/a.rs"),
        },
        0,
        5,
        serde_json::json!({"v": 1}),
    ));
    assert_eq!(state.assumptions.len(), 1);

    // Same id with different fact must not grow the map.
    state.assert_assumption(mk_assumption(
        "dup",
        AssumptionTarget::File {
            path: PathBuf::from("src/a.rs"),
        },
        0,
        5,
        serde_json::json!({"v": 2}),
    ));
    assert_eq!(state.assumptions.len(), 1);
}

#[test]
fn assumption_expiry_removes_past_ttl() {
    let mut state = SubstrateState::new();
    state.assert_assumption(mk_file_assumption("e1", "a.rs", 0, 2));
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
    // long  : posted_at=0, ttl=20 → expiry 20, remaining 15.
    // mid_a : posted_at=1, ttl=12 → expiry 13, remaining 8.
    // mid_b : posted_at=3, ttl=10 → expiry 13, remaining 8 (tiebreak — newer wins).
    // short : posted_at=2, ttl=5  → expiry 7,  remaining 2.
    state.assert_assumption(mk_file_assumption("long", "a.rs", 0, 20));
    state.assert_assumption(mk_file_assumption("mid-a", "b.rs", 1, 12));
    state.assert_assumption(mk_file_assumption("mid-b", "c.rs", 3, 10));
    state.assert_assumption(mk_file_assumption("short", "d.rs", 2, 5));

    let sorted = state.assumptions_sorted_by_remaining_ttl();
    let ids: Vec<&str> = sorted.iter().map(|(a, _)| a.id.as_str()).collect();
    assert_eq!(ids, vec!["long", "mid-b", "mid-a", "short"]);
    for pair in sorted.windows(2) {
        assert!(pair[0].1 >= pair[1].1);
    }
}

#[test]
fn assumptions_sorted_by_remaining_ttl_filters_expired() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    // Live: posted_at=2, ttl=10 → expiry 12, remaining 7.
    // Expired: posted_at=0, ttl=3 → expiry 3, current 5 → past TTL.
    state.assert_assumption(mk_file_assumption("live", "a.rs", 2, 10));
    state.assert_assumption(mk_file_assumption("expired", "b.rs", 0, 3));

    let sorted = state.assumptions_sorted_by_remaining_ttl();
    assert_eq!(sorted.len(), 1);
    assert_eq!(sorted[0].0.id, "live");
    assert_eq!(sorted[0].1, 7);
}
