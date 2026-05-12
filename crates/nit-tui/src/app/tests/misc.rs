//! Miscellaneous smoke tests that don't fit a more specific bucket.
//! Covers workspace_root construction and idle/busy probes via
//! `is_agent_busy`.

use super::*;

#[test]
fn default_state_workspace_is_dot() {
    let state = state_for_test();
    assert!(state.workspace_root.as_os_str() == ".");
}

#[test]
fn is_agent_busy_idle_lane_returns_false() {
    let state = state_for_test();
    assert!(!is_agent_busy(&state, "codex-main"));
}

#[test]
fn is_agent_busy_unknown_id_returns_false() {
    let state = state_for_test();
    assert!(!is_agent_busy(&state, "does-not-exist"));
}

#[test]
fn fresh_state_active_turns_is_empty() {
    let state = state_for_test();
    assert!(state.agents.active_turns.is_empty());
}

#[test]
fn agent_lane_kind_unknown_serialises_consistently() {
    let kind = nit_core::AgentLaneKind::Unknown;
    let json = serde_json::to_string(&kind).expect("serialize");
    assert_eq!(json, "\"unknown\"");
}
