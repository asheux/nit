//! Tests for the assumptions tab line builder: verifies the header chrome and
//! per-row emit for a list of `Assumption` records supplied via the
//! substrate state.

use super::*;
use nit_core::substrate::{Assumption, AssumptionTarget, SubstrateState};
use std::path::PathBuf;

fn mk_state_with_assumptions(assumptions: Vec<Assumption>) -> AppState {
    use nit_core::buffer::Buffer;
    let root = std::env::temp_dir().join(format!(
        "nit-assumptions-view-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::new(root, Buffer::empty("x", None), Buffer::empty("n", None));
    let mut substrate = SubstrateState::default();
    for assumption in assumptions {
        substrate.assumptions.insert(assumption.id.clone(), assumption);
    }
    state.substrate = substrate;
    state
}

fn mk_assumption(id: &str, posted_at_gen: u64, ttl_gens: u64) -> Assumption {
    Assumption {
        id: id.into(),
        target: AssumptionTarget::File {
            path: PathBuf::from("a.rs"),
        },
        fact: serde_json::Value::Null,
        posted_by: "agent-a".into(),
        posted_at_gen,
        ttl_gens,
        rationale: "test".into(),
    }
}

#[test]
fn build_lines_empty_has_header_and_hint() {
    let state = mk_state_with_assumptions(vec![]);
    let theme = Theme::default();
    let lines = build_lines(&state, &theme, 100);
    // summary + blank + column header + blank + empty hint = 5 lines
    assert_eq!(lines.len(), 5);
}

#[test]
fn build_lines_with_two_assumptions_emits_rows() {
    let assumptions = vec![mk_assumption("a1", 0, 5), mk_assumption("a2", 0, 3)];
    let state = mk_state_with_assumptions(assumptions);
    let theme = Theme::default();
    let lines = build_lines(&state, &theme, 100);
    // summary + blank + column header + 2 rows = 5 lines
    assert_eq!(lines.len(), 5);
}
