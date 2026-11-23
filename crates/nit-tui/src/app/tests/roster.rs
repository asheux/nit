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

#[test]
fn painted_recipient_button_and_model_pick_leave_multiline_draft_unsent() {
    use ratatui::{backend::TestBackend, Terminal};
    let mut state = state_for_test();
    state.center_tab = Some(nit_core::StationCenterTab::Chat);
    state.select_station_center(nit_core::StationCenterTab::Chat);
    state.agents.codex_cli_available = true;
    let draft = "review\nthe fixture";
    state.agents.chat_input = draft.into();
    state.agents.chat_input_cursor = 3;
    for (id, role, kind) in [
        ("local", "Local", nit_core::AgentLaneKind::Mock),
        ("codex-a", "Alpha", nit_core::AgentLaneKind::Codex),
        ("codex-b", "Beta", nit_core::AgentLaneKind::Codex),
    ] {
        state.agents.agents.push(nit_core::AgentLane {
            id: id.into(),
            role: role.into(),
            lane: format!("{kind:?}"),
            kind,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            shadow: false,
            last_message: String::new(),
        });
    }
    state.agents.selected_mission = Some("draft-mission".into());
    state.agents.agents[2].current_mission = Some("another-mission".into());
    state.agents.selected_agent = Some("local".into());
    let screen = Rect::new(0, 0, 176, 60);
    let area = layout::split_for_state(screen, &state);
    let swarm = SwarmRuntime::default();
    let theme = Theme::default();
    let mut term = Terminal::new(TestBackend::new(screen.width, screen.height)).unwrap();
    let mut paint_find = |state: &mut AppState, where_: Rect, needle: &str| {
        term.draw(|frame| {
            agent_ops_view::render(frame, area.job, state, &swarm, &theme);
            agent_console_view::render(frame, area.notes, state, &swarm, &theme);
        })
        .unwrap();
        for y in where_.y..where_.bottom() {
            let row: String = (where_.x..where_.right())
                .map(|x| term.backend().buffer().get(x, y).symbol())
                .collect();
            if let Some(i) = row.find(needle) {
                return (
                    where_.x + unicode_width::UnicodeWidthStr::width(&row[..i]) as u16,
                    y,
                );
            }
        }
        panic!("missing {needle}");
    };
    let mut input = InputState::new();
    let mut click = |x, y, state: &mut AppState| {
        handle_mouse_down(
            MouseEvent {
                kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            },
            screen,
            state,
            &mut input,
            &mut None,
            &theme,
        )
    };
    let (x, y) = paint_find(&mut state, area.notes, "[Select agent]");
    click(x - 1, y, &mut state);
    assert!(!state.agents.chat_needs_recipient);
    assert!(click(x + 1, y, &mut state));
    assert_eq!(state.focus, PaneId::JobOutput);
    assert!(state.agents.chat_needs_recipient);
    assert_eq!(state.agents.chat_input, draft);
    let (x, y) = paint_find(&mut state, area.job, "Beta");
    assert!(click(x + 1, y, &mut state));
    assert_eq!(state.focus, PaneId::Notes);
    assert_eq!(state.agents.selected_agent.as_deref(), Some("codex-b"));
    assert_eq!(
        state.agents.selected_context_mission(),
        Some("draft-mission")
    );
    assert!(!state.agents.chat_needs_recipient);
    assert_eq!(state.agents.chat_input, draft);
    assert!(state.agents.messages.is_empty() && state.agents.active_turns.is_empty());
}
