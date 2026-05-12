//! Chat input event-loop tests: paste / send / cursor navigation /
//! prompt history. Each scaffolds an `AppState` via `state_for_test*` and
//! drives the agent-station event handler directly.

use super::*;

#[test]
fn agent_chat_parses_all_prefix_as_broadcast() {
    let mut state = state_for_test();
    state.agents.chat_input = "@all hello swarm".into();
    let sent = push_chat_message(&mut state);
    assert_eq!(
        sent,
        Some((AgentChannel::Broadcast, "hello swarm".to_string()))
    );
    let last = state.agents.messages.last().expect("message");
    assert!(matches!(last.channel, AgentChannel::Broadcast));
    assert_eq!(last.text, "hello swarm");
}

#[test]
fn agent_chat_does_not_treat_allies_as_broadcast() {
    let mut state = state_for_test();
    state.agents.chat_input = "@allies hello".into();
    let sent = push_chat_message(&mut state);
    assert_eq!(
        sent,
        Some((AgentChannel::Agent, "@allies hello".to_string()))
    );
    let last = state.agents.messages.last().expect("message");
    assert!(matches!(last.channel, AgentChannel::Agent));
    assert_eq!(last.text, "@allies hello");
}

#[test]
fn agent_chat_accepts_input_and_sends_on_enter() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.console_scroll = 9;
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "hi");
    assert_eq!(state.agents.chat_input_cursor, 2);

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "");
    assert_eq!(state.agents.chat_input_cursor, 0);
    assert_eq!(state.agents.console_scroll, CONSOLE_SCROLL_BOTTOM);
    assert_eq!(state.agents.messages.len(), 1);
    assert_eq!(state.agents.messages[0].text, "hi");
}

#[test]
fn agent_chat_left_right_moves_cursor_and_inserts_at_cursor() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.chat_input = "helo".into();
    state.agents.chat_input_cursor = 4;
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Left, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "hello");

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Right, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input_cursor, 5);
}

#[test]
fn agent_chat_up_down_moves_cursor_between_lines() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.chat_input = "abcd\nxy\nlast".into();
    state.agents.chat_input_cursor = 3;
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input_cursor, 7);

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input_cursor, 10);

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input_cursor, 7);
}

#[test]
fn agent_chat_arrow_keys_move_cursor_not_thread_scroll() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.chat_input = "one\ntwo".into();
    state.agents.chat_input_cursor = 0;
    state.agents.console_scroll = 3;
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input_cursor, 4);
    assert_eq!(state.agents.console_scroll, 3);

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input_cursor, 0);
    assert_eq!(state.agents.console_scroll, 3);
}

#[test]
fn agent_chat_prompt_history_cycles_with_up_down() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    state.agents.chat_input = "first".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));

    state.agents.chat_input = "second".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "second");
    assert_eq!(state.agents.chat_input_cursor, 6);

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "first");
    assert_eq!(state.agents.chat_input_cursor, 5);

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "second");

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "");
    assert_eq!(state.agents.chat_input_cursor, 0);
}

#[test]
fn agent_chat_prompt_history_restores_draft() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    state.agents.chat_input = "one".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    state.agents.chat_input = "two".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));

    state.agents.chat_input = "draft".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "two");

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.chat_input, "draft");
    assert_eq!(state.agents.chat_input_cursor, 5);
}

#[test]
fn chat_paste_inserts_raw_text_without_sending_or_opening_command_prompt() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    let mut vitals = VitalsState::default();
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut fuzzy_runtime =
        FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
    let pasted = ":run now\n  keep this exactly\n@all is plain text";

    assert!(handle_paste_event(
        pasted,
        &mut state,
        &mut syntax,
        &mut fuzzy_runtime,
        &mut vitals
    ));
    assert_eq!(state.agents.chat_input, pasted);
    assert_eq!(state.agents.chat_input_cursor, pasted.chars().count());
    assert!(state.agents.messages.is_empty());
    assert!(state.command_line.is_none());
}

#[test]
fn chat_paste_normalizes_crlf_markdown_for_chat_box() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    let mut vitals = VitalsState::default();
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut fuzzy_runtime =
        FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
    let pasted = "# Plan\r\n- item 1\r\n```rust\r\nlet x = 1;\r\n```\r\n";

    assert!(handle_paste_event(
        pasted,
        &mut state,
        &mut syntax,
        &mut fuzzy_runtime,
        &mut vitals
    ));
    assert_eq!(
        state.agents.chat_input,
        "# Plan\n- item 1\n```rust\nlet x = 1;\n```\n"
    );
    assert!(!state.agents.chat_input.contains('\r'));
    assert_eq!(
        state.agents.chat_input_cursor,
        state.agents.chat_input.chars().count()
    );
    assert!(state.agents.messages.is_empty());
    assert!(state.command_line.is_none());
}

#[test]
fn insert_newline_preserves_indent_in_editor() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Insert;
    state.editor_buffer_mut().insert_str("    let x = 1;");

    let _ = apply_action(&mut state, Action::InsertNewline);
    assert_eq!(
        state.editor_buffer().content_as_string(),
        "    let x = 1;\n    "
    );
    assert_eq!(state.editor_buffer().cursor.line, 1);
    assert_eq!(state.editor_buffer().cursor.col, 4);
}

#[test]
fn agent_chat_send_preserves_pasted_formatting() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.chat_input = "  code block:\n    let x = 1;\n".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.messages.len(), 1);
    assert_eq!(
        state.agents.messages[0].text,
        "  code block:\n    let x = 1;\n"
    );
}

#[test]
fn agent_chat_send_preserves_markdown_text() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    let markdown = "# Plan\n- item 1\n- item 2\n```rust\nlet x = 1;\n```\n";
    state.agents.chat_input = markdown.into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.messages.len(), 1);
    assert_eq!(state.agents.messages[0].text, markdown);
}

#[test]
fn chat_thread_selection_starts_at_clicked_column() {
    let mut state = state_for_test();
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.console_scroll = 0;
    // Include the agent lane so the transcript renders in single-agent context and doesn't
    // waste horizontal space on redundant agent badges.
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
        agent_id: None,
        mission_id: None,
        text: "selection precision".into(),
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
    let text_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("thread area should be available");
    let lines = agent_console_view::thread_lines_for_selection(&state, text_area.width as usize);
    let line_idx = lines
        .iter()
        .position(|line| line.contains("precision"))
        .expect("precision line");
    let line = &lines[line_idx];
    let marker_byte = line.find("precision").expect("precision marker");
    let target_col = line[..marker_byte].chars().count();
    let click = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: text_area.x.saturating_add(target_col as u16),
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
    let selection = state.ui_selection.expect("selection should exist");
    assert_eq!(selection.pane, UiSelectionPane::AgentConsole);
    assert_eq!(selection.start_line, line_idx);
    assert_eq!(selection.start_col, target_col);
    let selected_char = line
        .chars()
        .nth(selection.start_col)
        .expect("selected char at cursor");
    assert_eq!(selected_char, 'p');
}

#[test]
fn esc_in_agent_chat_clears_thread_selection_before_chat_input() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.ui_selection = Some(nit_core::UiSelection {
        pane: UiSelectionPane::AgentConsole,
        start_line: 0,
        start_col: 0,
        end_line: 0,
        end_col: 1,
    });
    state.agents.chat_input = "draft message".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert!(state.ui_selection.is_none());
    assert_eq!(state.agents.chat_input, "draft message");
}

#[test]
fn esc_in_agent_chat_does_not_clear_chat_input_when_no_selection() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.ui_selection = None;
    state.agents.chat_input = "draft message".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(!handle_agent_station_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert!(state.ui_selection.is_none());
    assert_eq!(state.agents.chat_input, "draft message");
}
