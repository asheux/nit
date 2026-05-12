//! Editor / scratchpad / render-verify markdown tests. Each scaffolds an
//! AppState via state_for_test* and drives the key/mouse handler.

use super::*;

#[test]
fn editor_paste_normalizes_crlf_text() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Insert;
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
        state.editor_buffer().content_as_string(),
        "# Plan\n- item 1\n```rust\nlet x = 1;\n```\n"
    );
    assert!(!state.editor_buffer().content_as_string().contains('\r'));
}

#[test]
fn scratchpad_paste_normalizes_crlf_text() {
    let mut state = state_for_test();
    state.focus = PaneId::JobOutput;
    state.agents.dock_tab = AgentOpsTab::Scratchpad;
    state.mode = Mode::Insert;
    let mut vitals = VitalsState::default();
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut fuzzy_runtime =
        FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
    let pasted = "first\r\n    indented\r\n";

    assert!(handle_paste_event(
        pasted,
        &mut state,
        &mut syntax,
        &mut fuzzy_runtime,
        &mut vitals
    ));
    assert_eq!(
        state.notes_buffer().content_as_string(),
        "first\n    indented\n"
    );
    assert!(!state.notes_buffer().content_as_string().contains('\r'));
}

#[test]
fn scratchpad_in_agent_ops_accepts_insert_input() {
    let mut state = state_for_test();
    state.focus = PaneId::JobOutput;
    state.agents.dock_tab = AgentOpsTab::Scratchpad;
    state.mode = Mode::Insert;
    let mut input = InputState::new();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    // Scratchpad editing should flow through the normal action keymap.
    assert!(!handle_agent_station_key(
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    let action = map_key_to_action(
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        &state,
        &mut input,
    );
    assert_eq!(action, Some(Action::InsertChar('x')));
    let _ = apply_action(&mut state, Action::InsertChar('x'));
    assert!(state.notes_buffer().content_as_string().contains('x'));
}

#[test]
fn editor_ctrl_a_selects_all_and_ctrl_c_sets_yank() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Normal;
    state.editor_buffer_mut().insert_str("hello\nworld\n");
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut clipboard = None;

    assert!(handle_editor_buffer_shortcuts(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        &mut state,
        &mut syntax,
        &mut clipboard
    ));
    let len = state.editor_buffer().content_as_string().chars().count();
    assert_eq!(state.mode, Mode::Insert);
    assert_eq!(state.editor_buffer().selection_range(), Some((0, len)));

    assert!(handle_editor_buffer_shortcuts(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &mut state,
        &mut syntax,
        &mut clipboard
    ));
    assert_eq!(state.yank.as_deref(), Some("hello\nworld\n"));
    assert_eq!(state.yank_kind, YankKind::Line);
}

#[test]
fn editor_ctrl_x_cuts_selection_and_clears_buffer() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Normal;
    state.editor_buffer_mut().insert_str("hello");
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut clipboard = None;

    assert!(handle_editor_buffer_shortcuts(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        &mut state,
        &mut syntax,
        &mut clipboard
    ));
    assert!(handle_editor_buffer_shortcuts(
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        &mut state,
        &mut syntax,
        &mut clipboard
    ));

    assert_eq!(state.mode, Mode::Insert);
    assert_eq!(state.yank.as_deref(), Some("hello"));
    assert_eq!(state.yank_kind, YankKind::Char);
    assert_eq!(state.editor_buffer().content_as_string(), "");
    assert!(state.editor_buffer().selection_range().is_none());
}

#[test]
fn editor_ctrl_left_moves_by_word() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Insert;
    state.editor_buffer_mut().insert_str("hello world");
    state.editor_buffer_mut().move_end();
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut clipboard = None;

    assert!(handle_editor_buffer_shortcuts(
        KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
        &mut state,
        &mut syntax,
        &mut clipboard
    ));
    assert_eq!(state.editor_buffer().cursor.col, 6);
}

#[test]
fn editor_ctrl_backspace_deletes_word_left() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Insert;
    state.editor_buffer_mut().insert_str("hello world");
    state.editor_buffer_mut().move_end();
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut clipboard = None;

    assert!(handle_editor_buffer_shortcuts(
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL),
        &mut state,
        &mut syntax,
        &mut clipboard
    ));
    assert_eq!(state.mode, Mode::Insert);
    assert_eq!(state.editor_buffer().content_as_string(), "hello ");
    assert_eq!(state.editor_buffer().cursor.col, 6);
}

#[test]
fn scratchpad_ctrl_a_selects_all_and_ctrl_x_cuts() {
    let mut state = state_for_test();
    state.focus = PaneId::JobOutput;
    state.agents.dock_tab = AgentOpsTab::Scratchpad;
    state.mode = Mode::Insert;
    state.notes_buffer_mut().insert_str("scratch text");
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut clipboard = None;

    assert!(handle_editor_buffer_shortcuts(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        &mut state,
        &mut syntax,
        &mut clipboard
    ));
    let len = state.notes_buffer().content_as_string().chars().count();
    assert_eq!(state.notes_buffer().selection_range(), Some((0, len)));

    assert!(handle_editor_buffer_shortcuts(
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        &mut state,
        &mut syntax,
        &mut clipboard
    ));
    assert_eq!(state.notes_buffer().content_as_string(), "");
    assert_eq!(state.yank.as_deref(), Some("scratch text"));
}

#[test]
fn scratchpad_tab_cycles_ops_tabs_without_escaping_insert_mode() {
    let mut state = state_for_test();
    state.focus = PaneId::JobOutput;
    state.agents.dock_tab = AgentOpsTab::Scratchpad;
    state.mode = Mode::Insert;
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Roster);
    assert_eq!(state.mode, Mode::Normal);
}

#[test]
fn render_verify_markdown_includes_gate_summary_and_output_excerpt() {
    let markdown = render_verify_markdown(
        "mis-012",
        Some("rust-ci"),
        "auto",
        Some(&GateReport {
            overall_ok: false,
            gates: vec![GateReportGate {
                name: "clippy".into(),
                command: "cargo clippy --workspace --all-targets".into(),
                ok: false,
                status: Some("fail".into()),
                notes: Some("lint regression".into()),
            }],
        }),
        Some("warning: something went wrong"),
    );

    assert!(markdown.contains("# Verify"));
    assert!(markdown.contains("Mission: `mis-012`"));
    assert!(markdown.contains("Bundle: `rust-ci`"));
    assert!(markdown.contains("`clippy`: `FAIL`"));
    assert!(markdown.contains("`report.json`"));
    assert!(markdown.contains("warning: something went wrong"));
}
