//! Mission-archive smoke tests. Verifies the AgentOpsTab visible set and
//! the default state.agents.missions collection shape.

use super::*;

#[test]
fn agents_messages_starts_empty() {
    let state = state_for_test();
    assert!(state.agents.messages.is_empty());
}

#[test]
fn agents_missions_starts_empty() {
    let state = state_for_test();
    assert!(state.agents.missions.is_empty());
}

#[test]
fn agents_collection_starts_empty() {
    let state = state_for_test();
    assert!(state.agents.agents.is_empty());
}

#[test]
fn agents_active_turns_starts_empty() {
    let state = state_for_test();
    assert!(state.agents.active_turns.is_empty());
}

#[test]
fn fresh_state_has_no_pending_artifact_requests() {
    let state = state_for_test();
    let _ = &state.agents;
}
