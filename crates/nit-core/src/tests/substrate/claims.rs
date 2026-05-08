//! Claim conflict matrix, region/file overlap geometry, expiry, and the
//! TTL-sorted view used by the substrate overlay.

use super::*;

fn mk_claim(id: &str, kind: ClaimKind, target: ClaimTarget, gen: u64, ttl: u64) -> Claim {
    // Use the id as the owner so each test claim belongs to a distinct
    // agent — matches the conflict-test scenarios (cross-owner contention).
    // Same-owner re-assertion is explicitly a no-op in `claims_conflict`.
    Claim {
        id: id.into(),
        kind,
        target,
        claimed_by: id.into(),
        claimed_at_gen: gen,
        ttl_gens: ttl,
        rationale: "test".into(),
    }
}

fn claim_on(id: &str, kind: ClaimKind, path: &str, gen: u64, ttl: u64) -> Claim {
    mk_claim(
        id,
        kind,
        ClaimTarget::File {
            path: PathBuf::from(path),
        },
        gen,
        ttl,
    )
}

#[test]
fn claim_compat_exclusive_x_shared_conflict() {
    let a = claim_on("a", ClaimKind::ExclusiveWrite, "src/lib.rs", 0, 5);
    let b = claim_on("b", ClaimKind::SharedRead, "src/lib.rs", 0, 5);
    assert!(claims_conflict(&a, &b));
    assert!(claims_conflict(&b, &a));
}

#[test]
fn claim_compat_shared_x_shared_ok() {
    let a = claim_on("a", ClaimKind::SharedRead, "src/lib.rs", 0, 5);
    let b = claim_on("b", ClaimKind::SharedRead, "src/lib.rs", 0, 5);
    assert!(!claims_conflict(&a, &b));
}

#[test]
fn claim_compat_append_x_append_ok() {
    let a = claim_on("a", ClaimKind::AppendOnly, "src/lib.rs", 0, 5);
    let b = claim_on("b", ClaimKind::AppendOnly, "src/lib.rs", 0, 5);
    assert!(!claims_conflict(&a, &b));
}

#[test]
fn claim_compat_soft_never_conflicts() {
    let soft = claim_on("soft", ClaimKind::Soft, "src/lib.rs", 0, 5);
    let excl = claim_on("excl", ClaimKind::ExclusiveWrite, "src/lib.rs", 0, 5);
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
    state
        .assert_claim(claim_on("c1", ClaimKind::ExclusiveWrite, "a.rs", 0, 1))
        .unwrap();
    assert_eq!(state.claims.len(), 1);
    // TTL boundary is 0 + 1 = 1; gen=2 is past-expiry.
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
    state
        .assert_claim(claim_on("c1", ClaimKind::ExclusiveWrite, "a.rs", 0, 5))
        .unwrap();
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
    let live = claim_on("live", ClaimKind::ExclusiveWrite, "a.rs", 2, 10);
    let expired = claim_on("expired", ClaimKind::SharedRead, "b.rs", 0, 3);
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
    let short = claim_on("short", ClaimKind::ExclusiveWrite, "a.rs", 0, 1);
    let mid = claim_on("mid", ClaimKind::ExclusiveWrite, "b.rs", 0, 5);
    let long = claim_on("long", ClaimKind::ExclusiveWrite, "c.rs", 0, 10);
    state.claims.insert(short.id.clone(), short);
    state.claims.insert(mid.id.clone(), mid);
    state.claims.insert(long.id.clone(), long);

    let sorted = state.claims_sorted_by_remaining_ttl();
    let ids: Vec<&str> = sorted.iter().map(|(c, _)| c.id.as_str()).collect();
    assert_eq!(ids, vec!["long", "mid", "short"]);
    for pair in sorted.windows(2) {
        assert!(pair[0].1 >= pair[1].1);
    }
}

#[test]
fn claims_sorted_by_remaining_ttl_tiebreak_by_claimed_at_gen() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    // old: claimed_at=0, ttl=15 → expiry 15, remaining 10.
    // new: claimed_at=3, ttl=12 → expiry 15, remaining 10. Newer wins on tie.
    let old = claim_on("old", ClaimKind::ExclusiveWrite, "a.rs", 0, 15);
    let new = claim_on("new", ClaimKind::ExclusiveWrite, "b.rs", 3, 12);
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
    state
        .assert_claim(claim_on("c1", ClaimKind::ExclusiveWrite, "a.rs", 0, 5))
        .unwrap();

    let conflicting = Claim {
        id: "c2".into(),
        kind: ClaimKind::ExclusiveWrite,
        target: ClaimTarget::File { path },
        claimed_by: "agent-b".into(),
        claimed_at_gen: 0,
        ttl_gens: 5,
        rationale: "conflicting".into(),
    };
    let err = state
        .assert_claim(conflicting)
        .expect_err("should conflict");
    assert_eq!(err.conflicts.len(), 1);
    assert_eq!(err.conflicts[0].id, "c1");
    // Original claim must still be in the map; second must NOT have been inserted.
    assert_eq!(state.claims.len(), 1);
}
