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

#[test]
fn selecting_first_agent_preserves_skill_attached_to_empty_draft() {
    let mut state = state_for_test();
    state.focus = PaneId::GateMonitor;
    state.gate_monitor_sub_view = nit_core::GateMonitorSubView::Skills;
    assert!(state.agents.selected_context_agent().is_none());
    let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(gate_monitor_view::skills::handle_key(
        &mut state,
        key,
        Rect::new(0, 0, 60, 20)
    ));
    let draft = state.agents.chat_input.clone();
    assert!(draft.contains("nit skills genome-quality"));
    assert_eq!(state.focus, PaneId::Notes);
    assert_eq!(state.agents.chat_input_cursor, 0);

    state.agents.agents.push(nit_core::AgentLane {
        id: "model".into(),
        role: "agent".into(),
        lane: "chat".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    sync_roster_selected_agent(&mut state, 0);
    assert_eq!(state.agents.selected_context_agent(), Some("model"));
    assert_eq!(state.agents.chat_input, draft);
    assert_eq!(state.agents.chat_input_cursor, 0);
    assert!(state.agents.messages.is_empty() && state.agents.active_turns.is_empty());
}
