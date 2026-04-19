use super::*;
use nit_core::substrate::{Claim, ClaimKind, ClaimTarget, SubstrateState};
use std::path::PathBuf;

fn mk_state_with_claims(claims: Vec<Claim>) -> AppState {
    use nit_core::buffer::Buffer;
    let root = std::env::temp_dir().join(format!(
        "nit-claims-view-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::new(root, Buffer::empty("x", None), Buffer::empty("n", None));
    let mut substrate = SubstrateState::default();
    for c in claims {
        substrate.claims.insert(c.id.clone(), c);
    }
    state.substrate = substrate;
    state
}

fn mk_claim(id: &str, kind: ClaimKind, gen: u64, ttl: u64) -> Claim {
    Claim {
        id: id.into(),
        kind,
        target: ClaimTarget::File {
            path: PathBuf::from("a.rs"),
        },
        claimed_by: "agent-a".into(),
        claimed_at_gen: gen,
        ttl_gens: ttl,
        rationale: "test".into(),
    }
}

#[test]
fn build_lines_empty_has_header_and_hint() {
    let state = mk_state_with_claims(vec![]);
    let theme = Theme::default();
    let lines = build_lines(&state, &theme, 100);
    // summary + blank + column header + blank + empty hint = 5 lines
    assert_eq!(lines.len(), 5);
}

#[test]
fn build_lines_with_two_claims_emits_rows() {
    let claims = vec![
        mk_claim("c1", ClaimKind::ExclusiveWrite, 0, 5),
        mk_claim("c2", ClaimKind::SharedRead, 0, 3),
    ];
    let state = mk_state_with_claims(claims);
    let theme = Theme::default();
    let lines = build_lines(&state, &theme, 100);
    // summary + blank + column header + 2 rows = 5 lines
    assert_eq!(lines.len(), 5);
}
