//! Roster-picker smoke tests separate from the larger keymap/fuzzy flow.
//! Exercises AgentLaneKind variants and lane snapshot fields.

use super::*;

#[test]
fn lane_kind_codex_distinct_from_unknown() {
    assert_ne!(
        nit_core::AgentLaneKind::Codex,
        nit_core::AgentLaneKind::Unknown
    );
}

#[test]
fn lane_kind_claude_distinct_from_gemini() {
    assert_ne!(
        nit_core::AgentLaneKind::Claude,
        nit_core::AgentLaneKind::Gemini
    );
}

#[test]
fn lane_role_is_a_string_field() {
    let lane = nit_core::AgentLane {
        id: "x".to_string(),
        role: "clone".to_string(),
        lane: "swarm".to_string(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    };
    assert_eq!(lane.role, "clone");
}

#[test]
fn state_for_test_starts_with_empty_agents() {
    let state = state_for_test();
    assert!(state.agents.agents.is_empty());
}
