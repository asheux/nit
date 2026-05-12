//! Mouse / drag / wheel tests across agent_console, chat thread, and
//! user-bubble selection. Coordinates through map_agent_console_mouse.

use super::*;

#[test]
fn map_agent_console_mouse_maps_chat_thread_lines() {
    let mut state = state_for_test();
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    // In normal usage the roster provides a selected agent context; include a lane so the
    // thread renders in single-agent mode (no repeating badge) and preserves content width.
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: "idle".into(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "hello world".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 160,
        height: 48,
    };
    let layout = layout::split(screen);
    let text_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("thread area should be available");
    let mouse = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: text_area.x,
        row: text_area.y,
        modifiers: KeyModifiers::NONE,
    };

    let (line_idx, col, lines) = map_agent_console_mouse(mouse, screen, &state, false)
        .expect("mouse should map into chat thread");
    assert_eq!(line_idx, 0);
    assert_eq!(col, 0);
    let flattened = lines.concat();
    assert!(flattened.contains("[Planner]"));
    assert!(flattened.contains("done (see ARTIFACTS)"));
}

#[test]
fn clicking_agent_console_artifact_row_opens_matching_artifact_popup() {
    let mut state = state_for_test();
    state.agents.selected_agent = Some("planner".into());
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: "idle".into(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "hello world".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 160,
        height: 48,
    };
    let layout = layout::split(screen);
    let text_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("thread area should be available");
    let lines = agent_console_view::thread_lines_for_selection(&state, text_area.width as usize);
    let line_idx = lines
        .iter()
        .position(|line| line.contains("ARTIFACTS"))
        .expect("artifact row");
    let click = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: text_area
            .x
            .saturating_add(text_area.width.saturating_sub(1)),
        row: text_area.y.saturating_add(line_idx as u16),
        modifiers: KeyModifiers::NONE,
    };

    assert!(handle_mouse_down(
        click,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert!(state.agents.artifacts_popup_open);
    assert!(matches!(
        agent_ops_view::artifacts_popup_ref(&state, &SwarmRuntime::default(), 120),
        Some(agent_ops_view::ArtifactsPopupRef::Message { idx: 0 })
    ));
}

#[test]
fn clicking_outside_artifacts_popup_closes_it() {
    let mut state = state_for_test();
    state.agents.artifacts_popup_open = true;
    state.ui_selection = Some(UiSelection {
        pane: UiSelectionPane::ArtifactsPopup,
        start_line: 0,
        start_col: 0,
        end_line: 0,
        end_col: 0,
    });

    let mut input_state = InputState::new();
    input_state.mouse_select_anchor = Some(MouseSelectAnchor {
        target: MouseSelectTarget::Ui(UiSelectionPane::ArtifactsPopup),
        line: 0,
        col: 0,
    });
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 160,
        height: 48,
    };
    let area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
    let click = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: if area.x > 0 {
            area.x - 1
        } else {
            area.x.saturating_add(area.width)
        },
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };

    assert!(handle_mouse_down(
        click,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert!(!state.agents.artifacts_popup_open);
    assert_eq!(state.agents.artifacts_popup_scroll, 0);
    assert!(state.ui_selection.is_none());
    assert!(input_state.mouse_select_anchor.is_none());
}

#[test]
fn click_in_chat_input_box_moves_chat_cursor() {
    let mut state = state_for_test();
    state.agents.chat_input = "hello\nworld".into();
    state.agents.chat_input_cursor = 0;
    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 220,
        height: 42,
    };
    let layout = layout::split(screen);
    let input_area = agent_console_view::chat_input_text_area(layout.notes, &state)
        .expect("chat input area should be available");
    let down = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: input_area.x.saturating_add(2),
        row: input_area.y.saturating_add(1),
        modifiers: KeyModifiers::NONE,
    };
    assert!(handle_mouse_down(
        down,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert_eq!(state.focus, PaneId::Notes);
    assert_eq!(state.agents.chat_input_cursor, 8);
}

#[test]
fn click_in_chat_header_does_not_start_thread_selection() {
    let mut state = state_for_test();
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "select me".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 140,
        height: 42,
    };
    let layout = layout::split(screen);
    let context_click = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: layout.notes.x.saturating_add(3),
        row: layout.notes.y.saturating_add(1),
        modifiers: KeyModifiers::NONE,
    };
    assert!(handle_mouse_down(
        context_click,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert_eq!(state.focus, PaneId::Notes);
    assert!(state.ui_selection.is_none());
}

#[test]
fn clicking_swarm_clone_artifact_row_opens_matching_task_card() {
    let editor = nit_core::Buffer::from_str("editor", "", None);
    let notes = nit_core::Buffer::from_str("notes", "", None);
    let mut state = AppState::new(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        editor,
        notes,
    );
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state.agents.ops_viewport_width = 120;
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });

    let mut swarm = SwarmRuntime::default();
    let (mission_id, _dispatches) = swarm
        .start(
            &mut state,
            "planner".into(),
            vec!["planner".into()],
            SwarmSize::Count(2),
            Some("parallel".into()),
            None,
            "root".into(),
        )
        .expect("swarm start");
    let clone_id = format!("planner#swarm-{mission_id}-clone-01");

    let planner_message = format!(
        r#"
```json
{{
  "version": 2,
  "template": "parallel",
  "tasks": [
{{ "id": "t1", "agent_id": "{clone_id}", "title": "T1", "prompt": "DONE t1" }}
  ]
}}
```
"#
    );
    let planner_event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: planner_message,
    };
    planner_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &planner_event);

    let clone_event = AgentBusEvent::TurnCompleted {
        agent_id: clone_id.clone(),
        mission_id: Some(mission_id.clone()),
        thread_id: Some("thr-clone".into()),
        token_count: None,
        message: "clone output\n<SWARM_TASK_COMPLETE>".into(),
    };
    clone_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &clone_event);
    state.agents.selected_mission = Some(mission_id.clone());
    state.agents.console_scroll = 0;
    if let Some(mission_idx) = state
        .agents
        .missions
        .iter()
        .position(|mission| mission.id == mission_id)
    {
        state.agents.mission_selected = mission_idx;
    }

    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 160,
        height: 48,
    };
    let layout = layout::split(screen);
    let text_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("thread area should be available");
    let width = text_area.width as usize;
    let lines = agent_console_view::thread_lines_for_selection_with_swarm(&state, &swarm, width);
    let line_idx = lines
        .iter()
        .enumerate()
        .find_map(|(idx, _)| {
            let message_idx = agent_console_view::artifact_message_index_for_line_with_swarm(
                &state,
                Some(&swarm),
                width,
                idx,
            )?;
            (state.agents.messages[message_idx].agent_id.as_deref() == Some(clone_id.as_str()))
                .then_some(idx)
        })
        .expect("clone artifact row");
    let click = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: text_area.x.saturating_add(4),
        row: text_area.y.saturating_add(line_idx as u16),
        modifiers: KeyModifiers::NONE,
    };

    assert!(handle_mouse_down_with_swarm(
        &swarm,
        click,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert!(state.agents.artifacts_popup_open);
    assert!(matches!(
        agent_ops_view::artifacts_popup_ref(&state, &swarm, 120),
        Some(agent_ops_view::ArtifactsPopupRef::SwarmTask {
            mission_id: ref mid,
            ref task_id,
        }) if mid == &mission_id && task_id == "t1"
    ));
}

#[test]
fn mouse_wheel_in_chat_input_scrolls_input_not_thread() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.chat_input = (0..40).map(|i| format!("line-{i}\n")).collect();
    state.agents.chat_input_cursor = 0;
    state.agents.chat_input_scroll = 0;
    state.agents.console_scroll = 6;
    let mut fuzzy_runtime =
        FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 140,
        height: 42,
    };
    let layout = layout::split(screen);
    let input_area = agent_console_view::chat_input_text_area(layout.notes, &state)
        .expect("chat input area should be available");
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: input_area.x.saturating_add(1),
        row: input_area.y.saturating_add(1),
        modifiers: KeyModifiers::NONE,
    };
    assert!(handle_mouse_event(
        wheel,
        screen,
        &mut state,
        &mut fuzzy_runtime,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert!(state.agents.chat_input_scroll > 0);
    assert_eq!(state.agents.console_scroll, 6);
}

#[test]
fn mouse_wheel_in_chat_thread_clamps_at_bottom_without_wrap() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.missions.clear();
    state.agents.console_scroll = 0;
    state.agents.messages.clear();
    for i in 0..120 {
        state.agents.messages.push(AgentMessage {
            at: format!("10:00:{:02}", i % 60),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: None,
            text: format!("message-{i}"),
            prompt_msg_idx: None,
            kind: None,
        });
    }

    let mut fuzzy_runtime =
        FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 36,
    };
    let layout = layout::split(screen);
    let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("chat thread area should be available");
    let lines = agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
    let max_scroll = lines.len().saturating_sub(thread_area.height as usize);
    let wheel_down = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: thread_area.x.saturating_add(1),
        row: thread_area.y.saturating_add(1),
        modifiers: KeyModifiers::NONE,
    };

    for _ in 0..(max_scroll + 50) {
        assert!(handle_mouse_event(
            wheel_down,
            screen,
            &mut state,
            &mut fuzzy_runtime,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
    }
    assert_eq!(state.agents.console_scroll, max_scroll);

    assert!(handle_mouse_event(
        wheel_down,
        screen,
        &mut state,
        &mut fuzzy_runtime,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert_eq!(state.agents.console_scroll, max_scroll);
}

#[test]
fn user_bubble_selection_can_span_multiple_messages() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.missions.clear();
    state.agents.console_scroll = 0;
    state.agents.messages.clear();
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "alpha prompt".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "omega prompt".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 180,
        height: 42,
    };
    let layout = layout::split(screen);
    let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("chat thread area should be available");
    let lines = agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
    let (start_row, start_col) = lines
        .iter()
        .enumerate()
        .find_map(|(idx, line)| {
            line.find("alpha")
                .map(|byte| (idx, line[..byte].chars().count()))
        })
        .expect("alpha row");
    let (end_row, end_col) = lines
        .iter()
        .enumerate()
        .find_map(|(idx, line)| {
            line.find("omega")
                .map(|byte| (idx, line[..byte].chars().count() + "omega".chars().count()))
        })
        .expect("omega row");
    let down = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: thread_area.x.saturating_add(start_col as u16),
        row: thread_area.y.saturating_add(start_row as u16),
        modifiers: KeyModifiers::NONE,
    };
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        column: thread_area.x.saturating_add(end_col as u16),
        row: thread_area.y.saturating_add(end_row as u16),
        modifiers: KeyModifiers::NONE,
    };
    assert!(handle_mouse_down(
        down,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert!(handle_mouse_drag(
        drag,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    let yank = state.yank.clone().unwrap_or_default();
    assert!(yank.contains("alpha"));
    assert!(yank.contains("omega"));
}

#[test]
fn agent_console_mouse_drag_copies_selected_chat_text() {
    let mut state = state_for_test();
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "selection copy works".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 160,
        height: 48,
    };
    let layout = layout::split(screen);
    let text_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("thread area should be available");
    let down = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: text_area.x,
        // Agent messages are rendered as a combined callout row.
        row: text_area.y,
        modifiers: KeyModifiers::NONE,
    };
    assert!(handle_mouse_down(
        down,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        column: text_area.x.saturating_add(24),
        row: text_area.y.saturating_add(1),
        modifiers: KeyModifiers::NONE,
    };
    assert!(handle_mouse_drag(
        drag,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert_eq!(state.focus, PaneId::Notes);
    assert!(matches!(
        state.ui_selection.map(|s| s.pane),
        Some(UiSelectionPane::AgentConsole)
    ));
    assert!(state
        .yank
        .as_deref()
        .unwrap_or_default()
        .contains("done (see ARTIFACTS)"));
}
