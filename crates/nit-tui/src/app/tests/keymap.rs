//! Keymap routing tests: agent_ops tabs, command_prompt, petri,
//! file_tree, fuzzy_file_search, ctrl chords, parse_abort.

use super::*;

#[test]
fn agent_ops_space_does_not_toggle_pause() {
    let mut state = state_for_test();
    state.focus = PaneId::JobOutput;
    let mut input = InputState::new();
    let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty());
    let action = map_key_to_action(key, &state, &mut input);
    assert!(action.is_none());
}

#[test]
fn ctrl_focus_hotkeys_target_expected_panes() {
    let mut input = InputState::new();
    let state = state_for_test();
    let editor = map_key_to_action(
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::CONTROL),
        &state,
        &mut input,
    );
    assert_eq!(editor, Some(Action::FocusPane(PaneId::Editor)));
    let ops = map_key_to_action(
        KeyEvent::new(KeyCode::Char('2'), KeyModifiers::CONTROL),
        &state,
        &mut input,
    );
    assert_eq!(ops, Some(Action::FocusPane(PaneId::JobOutput)));
    let console = map_key_to_action(
        KeyEvent::new(KeyCode::Char('3'), KeyModifiers::CONTROL),
        &state,
        &mut input,
    );
    assert_eq!(console, Some(Action::FocusPane(PaneId::Notes)));
}

#[test]
fn ctrl_q_quits_but_ctrl_c_does_not() {
    let mut input = InputState::new();
    let state = state_for_test();
    let quit = map_key_to_action(
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        &state,
        &mut input,
    );
    assert_eq!(quit, Some(Action::Quit));
    let no_quit = map_key_to_action(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &state,
        &mut input,
    );
    assert_eq!(no_quit, None);
}

#[test]
fn ctrl_c_clears_chat_input_when_chat_focused() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.chat_input = "clear me".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
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
fn agent_ops_tabs_are_clickable() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Normal;
    state.agents.dock_tab = AgentOpsTab::Roster;

    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 200,
        height: 42,
    };
    let layout = layout::split(screen);
    let tabs_area = agent_ops_tab_bar_area(layout.job);
    let missions_start = "ROSTER".len() + 2; // two spaces between tabs
    let click = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: tabs_area
            .x
            .saturating_add(missions_start as u16)
            .saturating_add(1),
        row: tabs_area.y,
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
    assert_eq!(state.focus, PaneId::JobOutput);
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Missions);
}

#[test]
fn roster_row_click_is_aligned_with_rendered_body() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Normal;
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.agents = vec![
        nit_core::AgentLane {
            id: "agent-1".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            shadow: false,
            last_message: "idle".into(),
        },
        nit_core::AgentLane {
            id: "agent-2".into(),
            role: "Coder".into(),
            lane: "Lane B".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            shadow: false,
            last_message: "idle".into(),
        },
    ];
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Mock);
    state.agents.roster_selected = 1;

    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 200,
        height: 42,
    };
    let layout = layout::split(screen);

    // Click inside the Agent Ops roster table body area at the first agent row.
    let inner = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(layout.job);
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(1), // tabs
            ratatui::layout::Constraint::Length(1), // spacer below tabs
            ratatui::layout::Constraint::Min(1),    // body
            ratatui::layout::Constraint::Length(1), // footer hints
        ])
        .split(inner);
    let body_area = chunks[2];
    // The roster body now includes backend group headers; skip the first header row to land
    // on the first actual agent row.
    let agent_row_line = agent_ops_view::roster_body_offset(&state) + 1;
    let click = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: body_area.x.saturating_add(10),
        row: body_area.y.saturating_add(agent_row_line as u16),
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
    assert_eq!(state.agents.roster_selected, 0);
}

#[test]
fn roster_backend_row_click_toggles_backend_expansion() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Normal;
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.agents = vec![nit_core::AgentLane {
        id: "agent-1".into(),
        role: "Planner".into(),
        lane: "Local".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: "idle".into(),
    }];

    let mut input_state = InputState::new();
    let mut clipboard = None;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 200,
        height: 42,
    };
    let layout = layout::split(screen);
    let inner = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .inner(layout.job);
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(1),
            ratatui::layout::Constraint::Length(1),
        ])
        .split(inner);
    let body_area = chunks[2];
    let backend_row_line = agent_ops_view::roster_body_offset(&state);
    let click = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: body_area.x.saturating_add(6),
        row: body_area.y.saturating_add(backend_row_line as u16),
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
    assert!(state
        .agents
        .roster_expanded_backend_kinds
        .contains(&nit_core::AgentLaneKind::Mock));
    assert_eq!(
        state.agents.roster_selected_backend,
        Some(nit_core::AgentLaneKind::Mock)
    );

    assert!(handle_mouse_down(
        click,
        screen,
        &mut state,
        &mut input_state,
        &mut clipboard,
        &theme
    ));
    assert!(!state
        .agents
        .roster_expanded_backend_kinds
        .contains(&nit_core::AgentLaneKind::Mock));
    assert_eq!(
        state.agents.roster_selected_backend,
        Some(nit_core::AgentLaneKind::Mock)
    );
}

#[test]
fn roster_keyboard_navigation_includes_backend_rows() {
    let mut state = state_for_test();
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.agents = vec![
        nit_core::AgentLane {
            id: "gpt-5.4".into(),
            role: "gpt-5.4".into(),
            lane: "Codex".into(),
            kind: nit_core::AgentLaneKind::Codex,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            shadow: false,
            last_message: String::new(),
        },
        nit_core::AgentLane {
            id: "claude-sonnet-4".into(),
            role: "claude-sonnet-4".into(),
            lane: "Claude".into(),
            kind: nit_core::AgentLaneKind::Claude,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            shadow: false,
            last_message: String::new(),
        },
    ];
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Codex);
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Claude);
    state.agents.roster_selected = 1;

    assert!(move_agent_ops_selection(
        &mut state,
        &SwarmRuntime::default(),
        -1
    ));
    assert_eq!(
        agent_ops_view::roster_selected_row(&state),
        Some(agent_ops_view::RosterSelectableRow::Backend {
            backend: nit_core::AgentLaneKind::Claude,
        })
    );

    assert!(move_agent_ops_selection(
        &mut state,
        &SwarmRuntime::default(),
        -1
    ));
    assert_eq!(
        agent_ops_view::roster_selected_row(&state),
        Some(agent_ops_view::RosterSelectableRow::Agent { agent_idx: 0 })
    );
    assert_eq!(state.agents.roster_selected_backend, None);
}

#[test]
fn roster_enter_toggles_selected_backend() {
    let mut state = state_for_test();
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.agents.push(nit_core::AgentLane {
        id: "local".into(),
        role: "Local".into(),
        lane: "Local".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });

    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert_eq!(
        agent_ops_view::roster_selected_row(&state),
        Some(agent_ops_view::RosterSelectableRow::Backend {
            backend: nit_core::AgentLaneKind::Mock,
        })
    );
    assert!(handle_agent_ops_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm,
    ));
    assert!(state
        .agents
        .roster_expanded_backend_kinds
        .contains(&nit_core::AgentLaneKind::Mock));
    assert_eq!(
        state.agents.roster_selected_backend,
        Some(nit_core::AgentLaneKind::Mock)
    );

    assert!(handle_agent_ops_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm,
    ));
    assert!(!state
        .agents
        .roster_expanded_backend_kinds
        .contains(&nit_core::AgentLaneKind::Mock));
    assert_eq!(
        state.agents.roster_selected_backend,
        Some(nit_core::AgentLaneKind::Mock)
    );
}

#[test]
fn write_swarm_run_provenance_persists_final_report_markdown() {
    let mut state = state_for_test_in_workspace("swarm-final-report");
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
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

    let verify_event = AgentBusEvent::TurnCompleted {
        agent_id: clone_id,
        mission_id: Some(mission_id.clone()),
        thread_id: Some("thr-verify".into()),
        token_count: None,
        message: "verify output\n<SWARM_TASK_COMPLETE>".into(),
    };
    verify_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &verify_event);

    let final_report = "# Final Report\n\nShip it.\n";
    let report_event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: final_report.into(),
    };
    report_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &report_event);

    write_swarm_run_provenance(&state, &swarm, &mission_id).expect("write swarm provenance");

    let report_path = state
        .workspace_root
        .join(".nit")
        .join("swarm")
        .join(&mission_id)
        .join("report")
        .join("final.md");
    let saved = fs::read_to_string(report_path).expect("saved final report");
    assert!(saved.contains("# Final Report"));
    assert!(saved.contains("Ship it."));
}

#[test]
fn non_artifact_swarm_console_row_does_not_open_artifact_popup() {
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
        mission_id: Some(mission_id),
        thread_id: None,
        token_count: None,
        message: planner_message,
    };
    planner_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &planner_event);

    let lines = agent_console_view::thread_lines_for_selection_with_swarm(&state, &swarm, 120);
    let line_idx = lines
        .iter()
        .position(|line| line.contains("done") && !line.contains("ARTIFACTS"))
        .expect("plain done row");

    assert!(!maybe_open_artifact_popup_from_console_line(
        &mut state,
        Some(&swarm),
        120,
        line_idx
    ));
    assert!(!state.agents.artifacts_popup_open);
}

#[test]
fn wrapped_visualizer_side_selection_tracks_wrapped_rows() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    let config = nit_games::GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#,
    )
    .expect("parse config");
    state.games.config_preview = Some(nit_core::GamesConfigPreview {
        version: state.editor_buffer().version(),
        result: Ok(config.clone()),
    });

    let runtime = nit_games::RuntimeAcceleratorStats {
        backend: nit_games::RuntimeAcceleratorBackend::Metal,
        metal_matches: 913_936,
        metal_matches_per_batch: Some(262_144),
        metal_inflight_batches: Some(5),
        metal_policy_cache_path: Some(
            "/Users/nitrika/Library/Caches/dev.arcxlab.nit/games/metal-policy/apple_m4_max_1872106799188804901_v1.json"
                .into(),
        ),
        ..nit_games::RuntimeAcceleratorStats::default()
    };
    state.games.last_run = Some(nit_games::output::RunSummary {
        schema_version: nit_games::output::RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: "2026-03-11T23:19:22.86116Z".into(),
        run_id: "c3ca2b14966fcff".into(),
        seed: 42,
        config_text: String::new(),
        config: config.clone(),
        paths: nit_games::output::RunPaths {
            summary: Some(
                "/Users/nitrika/Projects/Configs/nit/runs/games/2026-03-11T23-19-22.86116Z__seed-42/run_summary.json"
                    .into(),
            ),
            events: None,
            history: None,
            definitions: None,
            results: None,
            config: None,
            analysis_dir: None,
        },
        strategies: Vec::new(),
        results: nit_games::output::TournamentResults {
            ranking: vec![nit_games::output::StrategyResult {
                id: "fsm_3495".into(),
                name: None,
                total_payoff: -1690,
                average_payoff: -0.884,
                adjusted_total_payoff: Some(-1690.0),
                adjusted_average_payoff: Some(-0.884),
                matches: 1,
                wins: 0,
                losses: 0,
                draws: 1,
                crashed: false,
                crash_count: 0,
                tm_metrics: None,
            }],
            pairwise: Vec::new(),
            dominance: Vec::new(),
        },
        event_log: None,
        history_log: None,
        runtime,
        run_dir: None,
    });

    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 240,
        height: 50,
    };
    let layout = layout::split(screen);
    let inner = Block::default()
        .borders(Borders::ALL)
        .inner(layout.visualizer);
    let layout_info = games_visualizer_view::layout_for_config(inner, &state, Some(&config));
    let side_area = layout_info.side.expect("side panel");
    let side_inner = Block::default().borders(Borders::ALL).inner(side_area);
    let lines = games_visualizer_view::build_side_lines(&state, &theme, side_inner.width as usize)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let accel_cache_idx = lines
        .iter()
        .position(|line| line.starts_with("accel_cache: "))
        .expect("accel_cache line");
    let target_line = (accel_cache_idx + 1..lines.len())
        .find(|&idx| lines[idx].starts_with("             "))
        .expect("wrapped continuation line");
    let line = &lines[target_line];
    let target_col = line
        .chars()
        .position(|ch| ch != ' ')
        .expect("non-space content on continuation line");
    let expected = line
        .chars()
        .nth(target_col)
        .expect("selected continuation char")
        .to_string();

    let mut input_state = InputState::new();
    let mut clipboard = None;
    let down = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: side_inner.x.saturating_add(target_col as u16),
        row: side_inner.y.saturating_add(target_line as u16),
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
        column: side_inner.x.saturating_add(target_col as u16 + 1),
        row: side_inner.y.saturating_add(target_line as u16),
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

    let selection = state.ui_selection.expect("selection should exist");
    assert_eq!(selection.pane, UiSelectionPane::VisualizerSide);
    assert_eq!(selection.start_line, target_line);
    assert_eq!(selection.end_line, target_line);
    assert_eq!(selection.start_col, target_col);
    assert_eq!(selection.end_col, target_col + 1);
    assert_eq!(state.yank.as_deref(), Some(expected.as_str()));
}

#[test]
fn scrolled_chat_selection_maps_to_visible_line_not_top() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.messages.clear();
    for i in 0..120 {
        state.agents.messages.push(AgentMessage {
            at: format!("10:00:{:02}", i % 60),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: format!("payload-{i:03}"),
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
        width: 140,
        height: 42,
    };
    let layout = layout::split(screen);
    let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("chat thread area should be available");
    let wheel_down = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: thread_area.x.saturating_add(1),
        row: thread_area.y.saturating_add(1),
        modifiers: KeyModifiers::NONE,
    };
    for _ in 0..18 {
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
    assert!(state.agents.console_scroll > 0);

    let lines = agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
    let visible_height = thread_area.height as usize;
    let visible_start = state.agents.console_scroll;
    let maybe_target = (0..visible_height)
        .filter_map(|row| {
            let idx = visible_start.saturating_add(row);
            lines.get(idx).map(|line| (row, idx, line))
        })
        .find(|(_, _, line)| line.contains("payl"));
    let (row_rel, expected_line_idx, line) = if let Some(target) = maybe_target {
        target
    } else {
        let visible = (0..visible_height)
            .filter_map(|row| {
                let idx = visible_start.saturating_add(row);
                lines.get(idx).cloned()
            })
            .collect::<Vec<_>>();
        panic!("payl line visible after scroll; visible={visible:?}");
    };
    let marker_byte = line.find("payl").expect("marker in visible line");
    let marker_col = line[..marker_byte].chars().count();
    let select_col = marker_col + 1;
    let expected_char = line
        .chars()
        .nth(select_col)
        .expect("character at selection point")
        .to_string();

    let down = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: thread_area.x.saturating_add(select_col as u16),
        row: thread_area.y.saturating_add(row_rel as u16),
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
    let selection = state.ui_selection.expect("selection exists");
    assert_eq!(selection.start_line, expected_line_idx);
    assert_eq!(selection.start_col, select_col);

    let drag = MouseEvent {
        kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        column: down.column.saturating_add(1),
        row: down.row,
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
    assert_eq!(state.yank.as_deref(), Some(expected_char.as_str()));
}

#[test]
fn scrolled_user_bubble_selection_can_span_multiple_messages() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.missions.clear();
    state.agents.console_scroll = 0;
    state.agents.messages.clear();
    for i in 0..60 {
        state.agents.messages.push(AgentMessage {
            at: format!("10:00:{:02}", i % 60),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: format!("user-prompt-{i:02}"),
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
        width: 180,
        height: 42,
    };
    let layout = layout::split(screen);
    let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("chat thread area should be available");
    let wheel_down = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: thread_area.x.saturating_add(1),
        row: thread_area.y.saturating_add(1),
        modifiers: KeyModifiers::NONE,
    };
    for _ in 0..12 {
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
    let lines = agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
    let visible_start = state.agents.console_scroll;
    let visible_end = visible_start.saturating_add(thread_area.height as usize);
    let visible = lines
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx >= visible_start && *idx < visible_end)
        .collect::<Vec<_>>();
    let (start_row_rel, start_col, _) = visible
        .iter()
        .find_map(|(idx, line)| {
            line.find("user-prompt-").map(|byte| {
                (
                    idx.saturating_sub(visible_start),
                    line[..byte].chars().count(),
                    *idx,
                )
            })
        })
        .expect("visible start prompt");
    let (end_row_rel, end_col, _) = visible
        .iter()
        .rev()
        .find_map(|(idx, line)| {
            line.find("user-prompt-").map(|byte| {
                (
                    idx.saturating_sub(visible_start),
                    line[..byte].chars().count() + 8,
                    *idx,
                )
            })
        })
        .expect("visible end prompt");
    let down = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: thread_area.x.saturating_add(start_col as u16),
        row: thread_area.y.saturating_add(start_row_rel as u16),
        modifiers: KeyModifiers::NONE,
    };
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        column: thread_area.x.saturating_add(end_col as u16),
        row: thread_area.y.saturating_add(end_row_rel as u16),
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
    assert!(yank.contains("user-prompt-"));
    let unique_hits = yank.matches("user-prompt-").count();
    assert!(
        unique_hits >= 2,
        "expected >= 2 prompts in yank, got {unique_hits} from {yank:?}"
    );
}

#[test]
fn vertical_drag_across_user_bubbles_includes_end_message_text() {
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
        text: "first message has a wider bubble".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "short".into(),
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
            line.find("first")
                .map(|byte| (idx, line[..byte].chars().count() + 2))
        })
        .expect("first row");
    let end_row = lines
        .iter()
        .enumerate()
        .find_map(|(idx, line)| line.contains("short").then_some(idx))
        .expect("short row");
    let end_col = lines
        .get(end_row)
        .and_then(|line| {
            line.find("short")
                .map(|byte| line[..byte].chars().count() + "short".chars().count())
        })
        .expect("short col");

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
    assert!(
        yank.contains("short"),
        "end message text missing from yank: {yank:?}"
    );
}

#[test]
fn reverse_vertical_drag_across_user_bubbles_keeps_both_messages() {
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
        text: "first message has a wider bubble".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "short".into(),
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
            line.find("short")
                .map(|byte| (idx, line[..byte].chars().count() + "short".chars().count()))
        })
        .expect("short row");
    let end_row = lines
        .iter()
        .enumerate()
        .find_map(|(idx, line)| line.contains("first").then_some(idx))
        .expect("first row");
    let end_payload_start = user_bubble_payload_start_col(
        lines
            .get(end_row)
            .expect("line for first message payload should exist"),
    )
    .expect("payload start for first message");
    let end_col = end_payload_start.saturating_sub(1);

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
    assert!(
        yank.contains("first"),
        "first message missing from yank: {yank:?}"
    );
    assert!(
        yank.contains("short"),
        "second message missing from yank: {yank:?}"
    );
}

#[test]
fn scrolled_reverse_drag_across_user_bubbles_keeps_visible_messages() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.missions.clear();
    state.agents.console_scroll = 0;
    state.agents.messages.clear();
    for i in 0..80 {
        state.agents.messages.push(AgentMessage {
            at: format!("10:00:{:02}", i % 60),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: format!("user-prompt-{i:02}"),
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
        width: 180,
        height: 42,
    };
    let layout = layout::split(screen);
    let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
        .expect("chat thread area should be available");
    let wheel_down = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: thread_area.x.saturating_add(1),
        row: thread_area.y.saturating_add(1),
        modifiers: KeyModifiers::NONE,
    };
    for _ in 0..16 {
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

    let lines = agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
    let visible_start = state.agents.console_scroll;
    let visible_end = visible_start.saturating_add(thread_area.height as usize);
    let visible_prompt_rows = lines
        .iter()
        .enumerate()
        .filter(|(idx, line)| {
            *idx >= visible_start && *idx < visible_end && line.contains("user-prompt-")
        })
        .collect::<Vec<_>>();
    assert!(
        visible_prompt_rows.len() >= 2,
        "need at least two visible prompt rows after scroll"
    );
    let (start_abs_row, start_line) = *visible_prompt_rows.last().expect("last visible prompt row");
    let (end_abs_row, end_line) = *visible_prompt_rows
        .first()
        .expect("first visible prompt row");
    let start_row_rel = start_abs_row.saturating_sub(visible_start);
    let end_row_rel = end_abs_row.saturating_sub(visible_start);
    let start_col = start_line
        .find("user-prompt-")
        .map(|byte| start_line[..byte].chars().count() + "user-prompt-".chars().count())
        .expect("start prompt marker");
    let end_payload_start =
        user_bubble_payload_start_col(end_line).expect("payload start in end prompt row");
    let end_col = end_payload_start.saturating_sub(1);

    let down = MouseEvent {
        kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: thread_area.x.saturating_add(start_col as u16),
        row: thread_area.y.saturating_add(start_row_rel as u16),
        modifiers: KeyModifiers::NONE,
    };
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        column: thread_area.x.saturating_add(end_col as u16),
        row: thread_area.y.saturating_add(end_row_rel as u16),
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
    let hits = yank.matches("user-prompt-").count();
    assert!(
        hits >= 2,
        "expected >=2 prompt hits in yank after scrolled reverse drag, got {hits}: {yank:?}"
    );
}

#[test]
fn agent_console_selection_strips_user_bubble_edges_from_clipboard() {
    let mut state = state_for_test();
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.console_scroll = 0;
    state.agents.messages.clear();
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "hello".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 36,
    };
    let layout = layout::split(screen);
    let thread_area =
        agent_console_view::thread_text_area(layout.notes, &state).expect("thread area");
    let lines = agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
    let (line_idx, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains("hello"))
        .expect("hello bubble line");
    let line_len = line.chars().count();
    let selection = UiSelection {
        pane: UiSelectionPane::AgentConsole,
        start_line: line_idx,
        start_col: 0,
        end_line: line_idx,
        end_col: line_len,
    };
    let text = selection_text_agent_console(&lines, selection);
    assert_eq!(text, "hello");
}

#[test]
fn agent_console_selection_does_not_strip_markdown_table_pipes_in_user_prompt() {
    let mut state = state_for_test();
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.console_scroll = 0;
    state.agents.messages.clear();
    state.agents.agents.push(nit_core::AgentLane {
        id: "coder".into(),
        role: "Coder".into(),
        lane: "Lane B".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Running,
        heartbeat_age_secs: 1,
        queue_len: 1,
        current_mission: None,
        shadow: false,
        last_message: "active".into(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "intro\n| table row |".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 140,
        height: 42,
    };
    let layout = layout::split(screen);
    let thread_area =
        agent_console_view::thread_text_area(layout.notes, &state).expect("thread area");
    let lines = agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
    let (line_idx, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains("| table row |"))
        .expect("table row line");
    let line_len = line.chars().count();
    let selection = UiSelection {
        pane: UiSelectionPane::AgentConsole,
        start_line: line_idx,
        start_col: 0,
        end_line: line_idx,
        end_col: line_len,
    };
    let text = selection_text_agent_console(&lines, selection);
    assert!(
        text.contains("| table row |"),
        "unexpected stripped text: {text:?}"
    );
    assert_eq!(text, "| table row |");
}

#[test]
fn agent_ops_left_right_arrows_switch_tabs() {
    let mut state = state_for_test();
    state.focus = PaneId::JobOutput;
    state.agents.dock_tab = AgentOpsTab::Roster;
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Right, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Missions);

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Left, KeyModifiers::empty()),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Roster);
}

#[test]
fn job_pause_key_matches_ctrl_space_and_f6() {
    let ctrl_space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
    assert!(is_job_pause_key(&ctrl_space));

    let f6 = KeyEvent::new(KeyCode::F(6), KeyModifiers::empty());
    assert!(is_job_pause_key(&f6));

    // Depending on terminal + crossterm backend, Ctrl+Space can also arrive as NULL.
    let nul_code = KeyEvent::new(KeyCode::Char('\u{0}'), KeyModifiers::empty());
    assert!(is_job_pause_key(&nul_code));

    let null_code = KeyEvent::new(KeyCode::Null, KeyModifiers::empty());
    assert!(is_job_pause_key(&null_code));
}

#[test]
fn command_prompt_open_key_matches_colon_and_shift_semicolon() {
    let colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty());
    assert!(is_command_prompt_open_key(&colon));

    let semicolon_shift = KeyEvent::new(KeyCode::Char(';'), KeyModifiers::SHIFT);
    assert!(is_command_prompt_open_key(&semicolon_shift));
}

#[test]
fn command_prompt_open_key_does_not_trigger_in_insert_mode() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Insert;
    let mut input = InputState::new();

    let colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::SHIFT);
    assert_eq!(
        map_key_to_action(colon, &state, &mut input),
        Some(Action::InsertChar(':'))
    );
}

#[test]
fn command_prompt_open_key_triggers_in_normal_mode() {
    let mut state = state_for_test();
    state.focus = PaneId::Editor;
    state.mode = Mode::Normal;
    let mut input = InputState::new();

    let colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty());
    assert_eq!(
        map_key_to_action(colon, &state, &mut input),
        Some(Action::CommandPromptOpen)
    );
}

#[test]
fn petri_show_key_matches_ctrl_caret_terminal_variants() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.games.running = true;
    state.games.petri_hidden = true;

    let ctrl_six = KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL);
    assert!(is_petri_show_key(&ctrl_six, &state));

    let ctrl_caret = KeyEvent::new(KeyCode::Char('^'), KeyModifiers::CONTROL);
    assert!(is_petri_show_key(&ctrl_caret, &state));

    let rs_control_char = KeyEvent::new(KeyCode::Char('\u{1e}'), KeyModifiers::empty());
    assert!(is_petri_show_key(&rs_control_char, &state));
}

#[test]
fn petri_show_key_allows_done_games_session() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.games.running = false;
    state.games.status = nit_core::GamesStatus::Done;
    state.games.petri_hidden = true;
    state.games.petri_lines = vec!["Status: Done".into()];

    let ctrl_six = KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL);
    assert!(is_petri_show_key(&ctrl_six, &state));
    assert!(games_petri_active(&state));
    state.games.petri_hidden = false;
    assert!(games_petri_visible(&state));
}

#[test]
fn map_key_to_action_can_show_hidden_games_petri_from_editor_focus() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.focus = PaneId::Editor;
    state.mode = Mode::Insert;
    state.games.running = true;
    state.games.petri_hidden = true;
    let mut input = InputState::new();

    let ctrl_six = KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL);
    assert_eq!(
        map_key_to_action(ctrl_six, &state, &mut input),
        Some(Action::GamesShow)
    );
}

#[test]
fn games_history_open_key_matches_ctrl_star_variants() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.games.running = true;

    let ctrl_star = KeyEvent::new(KeyCode::Char('*'), KeyModifiers::CONTROL);
    assert!(is_games_history_open_key(&ctrl_star, &state));

    let ctrl_shift_star = KeyEvent::new(
        KeyCode::Char('*'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(is_games_history_open_key(&ctrl_shift_star, &state));

    let ctrl_eight = KeyEvent::new(KeyCode::Char('8'), KeyModifiers::CONTROL);
    assert!(is_games_history_open_key(&ctrl_eight, &state));
}

#[test]
fn file_tree_does_not_consume_command_or_help_toggle_keys() {
    let mut state = state_for_test();
    state.file_tree.open = true;
    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let area = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };

    let colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty());
    assert!(!handle_file_tree_key(&colon, &mut state, &mut syntax, area));

    let help = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
    assert!(!handle_file_tree_key(&help, &mut state, &mut syntax, area));

    state.app_kind = AppKind::Games;
    state.games.running = true;
    state.games.petri_hidden = true;
    let show_hidden = KeyEvent::new(KeyCode::Char('\u{1e}'), KeyModifiers::empty());
    assert!(!handle_file_tree_key(
        &show_hidden,
        &mut state,
        &mut syntax,
        area
    ));

    let history = KeyEvent::new(KeyCode::Char('8'), KeyModifiers::CONTROL);
    assert!(!handle_file_tree_key(
        &history,
        &mut state,
        &mut syntax,
        area
    ));
}

#[test]
fn file_tree_opens_selected_file_when_current_editor_buffer_is_dirty() {
    let mut state = state_for_test_in_workspace("file-tree-dirty-open");
    let file_a = state.workspace_root.join("a.txt");
    let file_b = state.workspace_root.join("b.txt");
    fs::write(&file_a, "alpha").expect("write a");
    fs::write(&file_b, "beta").expect("write b");
    state.buffers[state.active_editor_buffer_id] =
        nit_core::Buffer::from_str("a.txt", "alpha", Some(file_a.clone()));
    state.editor_buffer_mut().insert_char('!');
    state.file_tree.open = true;
    state.file_tree.rows = vec![nit_core::FileTreeRow {
        text: "b.txt".into(),
        path: file_b.clone(),
        kind: nit_core::FileTreeKind::File,
        depth: 0,
    }];
    state.file_tree.selected = 0;

    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let area = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());

    assert!(handle_file_tree_key(&enter, &mut state, &mut syntax, area));
    assert!(!state.file_tree.open);
    assert_eq!(state.editor_buffer().path(), Some(&file_b));
    assert_eq!(state.editor_buffer().content_as_string(), "beta");

    let original = state.buffer(0).expect("original editor buffer");
    assert_eq!(original.path(), Some(&file_a));
    assert!(original.is_dirty());
}

#[test]
fn fuzzy_file_search_opens_selected_file_when_current_editor_buffer_is_dirty() {
    let mut state = state_for_test_in_workspace("fuzzy-dirty-open");
    let file_a = state.workspace_root.join("a.txt");
    let file_b = state.workspace_root.join("b.txt");
    fs::write(&file_a, "alpha").expect("write a");
    fs::write(&file_b, "beta").expect("write b");
    state.buffers[state.active_editor_buffer_id] =
        nit_core::Buffer::from_str("a.txt", "alpha", Some(file_a.clone()));
    state.editor_buffer_mut().insert_char('!');
    state.fuzzy_search.open = true;
    state.fuzzy_search.mode = SearchMode::Files;
    state.fuzzy_search.file_results = vec![nit_core::SearchResultFile {
        rel_path: "b.txt".into(),
        abs_path: file_b.clone(),
        score: 42,
        matched_indices: vec![0],
    }];
    state.fuzzy_search.selected = 0;

    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let mut fuzzy_runtime =
        FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());

    assert!(handle_fuzzy_search_key(
        &enter,
        &mut state,
        &mut syntax,
        &mut fuzzy_runtime,
        screen
    ));
    assert!(!state.fuzzy_search.open);
    assert_eq!(state.editor_buffer().path(), Some(&file_b));
    assert_eq!(state.editor_buffer().content_as_string(), "beta");

    let original = state.buffer(0).expect("original editor buffer");
    assert_eq!(original.path(), Some(&file_a));
    assert!(original.is_dirty());
}

#[test]
fn toggle_roster_priority_supports_claude_and_gemini_models() {
    let mut state = state_for_test();
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Claude);
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Gemini);
    state.agents.agents.push(nit_core::AgentLane {
        id: "claude-sonnet-4".into(),
        role: "claude-sonnet-4".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "gemini-2.5-pro".into(),
        role: "gemini-2.5-pro".into(),
        lane: "Gemini".into(),
        kind: nit_core::AgentLaneKind::Gemini,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });

    state.agents.roster_selected = 0;
    assert!(toggle_roster_priority(&mut state));
    assert!(state
        .agents
        .swarm_priority_agent_ids
        .contains("claude-sonnet-4"));

    state.agents.roster_selected = 1;
    assert!(toggle_roster_priority(&mut state));
    assert!(state
        .agents
        .swarm_priority_agent_ids
        .contains("gemini-2.5-pro"));
}

#[test]
fn background_work_active_when_games_running_or_loading() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    assert!(!is_background_work_active(&state));

    state.games.running = true;
    assert!(is_background_work_active(&state));

    state.games.running = false;
    state.games.run_browser.loading = true;
    assert!(is_background_work_active(&state));
}

#[test]
fn background_work_active_when_status_text_is_busy() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.status = Some("Games analysis started".into());
    assert!(is_background_work_active(&state));

    state.status = Some("Games tournament completed".into());
    assert!(!is_background_work_active(&state));
}

#[test]
fn log_line_vitals_do_not_refresh_job_heartbeat_without_real_activity() {
    let mut vitals = VitalsState::default();
    let start = Instant::now();
    vitals.record_job_event(start);

    let later = start + Duration::from_secs(3);
    record_log_line_vitals(&mut vitals, later, "INFO just a message");

    let age = vitals.job_hb.age(later).unwrap_or_default();
    assert!(age >= Duration::from_secs(3));
}

#[test]
fn status_looks_busy_matches_expected_keywords() {
    assert!(status_looks_busy("Games analysis started"));
    assert!(status_looks_busy("Preparing run config..."));
    assert!(status_looks_busy("Loading replay..."));
    assert!(!status_looks_busy("Games tournament completed"));
    assert!(!status_looks_busy("Saved"));
}

#[test]
fn vitals_smoke_games_busy_phase_keeps_ecg_alive_before_run() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.games.pending_run = true;
    state.status = Some("Preparing run config...".into());

    let mut vitals = VitalsState::default();
    let mut now = Instant::now();
    let dt = Duration::from_millis(100);
    let mut last_busy_pulse = now;
    let mut max_sample = 0u64;
    let mut last_snapshot = vitals.tick(
        now,
        dt,
        is_lab_job_running(&state),
        current_agent_state(&state),
    );

    for _ in 0..40 {
        now += dt;
        if is_background_work_active(&state)
            && !is_lab_job_running(&state)
            && now.saturating_duration_since(last_busy_pulse) >= BUSY_PULSE_INTERVAL
        {
            vitals.record_job_event(now);
            last_busy_pulse = now;
        }
        last_snapshot = vitals.tick(
            now,
            dt,
            is_lab_job_running(&state),
            current_agent_state(&state),
        );
        max_sample = max_sample.max(*last_snapshot.ecg_samples.last().unwrap_or(&0));
    }

    assert!(
        max_sample >= 30,
        "expected busy pulses to animate ECG before run, got {max_sample}"
    );
    assert_eq!(
        last_snapshot.criticality,
        crate::vitals::LabCriticality::Idle
    );
}

#[test]
fn vitals_smoke_games_run_then_stall_hits_crit_boundary() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.games.running = true;
    state.games.paused = false;

    let mut vitals = VitalsState::default();
    let mut now = Instant::now();
    let dt = Duration::from_millis(100);

    let mut snapshot = vitals.tick(
        now,
        dt,
        is_lab_job_running(&state),
        current_agent_state(&state),
    );
    for _ in 0..30 {
        now += dt;
        vitals.record_job_event(now);
        snapshot = vitals.tick(
            now,
            dt,
            is_lab_job_running(&state),
            current_agent_state(&state),
        );
    }

    assert!(snapshot.hb_age.unwrap_or(Duration::MAX) < Duration::from_secs(1));
    assert_ne!(snapshot.criticality, crate::vitals::LabCriticality::Crit);

    for _ in 0..120 {
        now += dt;
        snapshot = vitals.tick(
            now,
            dt,
            is_lab_job_running(&state),
            current_agent_state(&state),
        );
    }

    assert!(snapshot.hb_age.unwrap_or_default() >= Duration::from_secs(10));
    assert_eq!(snapshot.criticality, crate::vitals::LabCriticality::Crit);
}

#[test]
fn artifact_popup_context_overrides_agent_and_mission() {
    let mut state = state_for_test();

    // Set up two agents.
    state.agents.agents.push(nit_core::AgentLane {
        id: "base-model".into(),
        role: "Base".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "clone-agent".into(),
        role: "Clone".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });

    // Push a message from the clone in mis-002.
    state.agents.messages.push(nit_core::AgentMessage {
        at: "t+1".into(),
        channel: nit_core::AgentChannel::Agent,
        agent_id: Some("clone-agent".into()),
        mission_id: Some("mis-002".into()),
        text: "Clone result".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    // Roster has base-model selected, mis-001 selected.
    state.agents.selected_agent = Some("base-model".into());
    state.agents.selected_mission = Some("mis-001".into());

    // Before override: context points to base-model + mis-001.
    assert_eq!(state.agents.selected_context_agent(), Some("base-model"));
    assert_eq!(state.agents.selected_context_mission(), Some("mis-001"));

    // Simulate what the artifacts popup Enter handler does:
    // override agent and mission from the artifact.
    let prev_agent = state.agents.selected_agent.clone();
    let prev_mission = state.agents.selected_mission.clone();
    state.agents.selected_agent = Some("clone-agent".into());
    state.agents.selected_mission = Some("mis-002".into());

    // After override: context points to clone-agent + mis-002.
    assert_eq!(state.agents.selected_context_agent(), Some("clone-agent"));
    assert_eq!(state.agents.selected_context_mission(), Some("mis-002"));

    // Restore: context reverts.
    state.agents.selected_agent = prev_agent;
    state.agents.selected_mission = prev_mission;
    assert_eq!(state.agents.selected_context_agent(), Some("base-model"));
    assert_eq!(state.agents.selected_context_mission(), Some("mis-001"));
}

#[test]
fn artifact_popup_dispatches_idle_agent_even_when_other_agents_busy() {
    let mut state = state_for_test();
    let mut vitals = VitalsState::default();
    let codex = CodexRunner::spawn(CodexRuntimeMode::Exec, CodexRunnerConfig::default(), None);

    // Agent A is busy (running a turn).
    state.agents.agents.push(nit_core::AgentLane {
        id: "agent-a".into(),
        role: "A".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.active_turns.insert(
        "agent-a".into(),
        nit_core::state::AgentTurnState {
            started_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            last_output_at: Instant::now(),
            stage: None,
        },
    );
    state
        .agents
        .codex_effective_context_window_tokens
        .insert("agent-a".into(), 128_000);

    // Agent B is idle (the artifact's agent).
    state.agents.agents.push(nit_core::AgentLane {
        id: "agent-b".into(),
        role: "B".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state
        .agents
        .codex_effective_context_window_tokens
        .insert("agent-b".into(), 128_000);

    // Agent A is busy — verify.
    assert!(is_agent_busy(&state, "agent-a"));
    // Agent B is idle — verify.
    assert!(!is_agent_busy(&state, "agent-b"));

    // Direct dispatch to agent-b (simulating what the artifact popup does).
    // Even though agent-a is busy, agent-b should dispatch immediately.
    maybe_dispatch_codex_turn(
        &mut state,
        &mut vitals,
        Some(&codex),
        Some("agent-b".into()),
        None,
        "question about artifact".into(),
        true,
    );

    // Agent B should now have an active turn (dispatched, not queued).
    assert!(
        state.agents.active_turns.contains_key("agent-b"),
        "agent-b should be dispatched immediately, not queued"
    );
    // Agent A should still be running (unchanged).
    assert!(state.agents.active_turns.contains_key("agent-a"));
}

#[test]
fn artifact_popup_queues_when_artifact_agent_is_busy() {
    let mut state = state_for_test();
    let mut vitals = VitalsState::default();

    // Agent B is busy (has an active turn).
    state.agents.agents.push(nit_core::AgentLane {
        id: "agent-b".into(),
        role: "B".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.active_turns.insert(
        "agent-b".into(),
        nit_core::state::AgentTurnState {
            started_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            last_output_at: Instant::now(),
            stage: None,
        },
    );

    assert!(is_agent_busy(&state, "agent-b"));

    // Enqueue for agent-b since it's busy.
    enqueue_codex_turn(
        &mut state,
        &mut vitals,
        Some("agent-b".into()),
        None,
        "follow-up".into(),
        Some(0),
    );

    // Should be queued, not dispatched.
    assert_eq!(state.agents.queued_codex_turns.len(), 1);
    assert_eq!(state.agents.queued_codex_turns[0].agent_id, "agent-b");
}

// Shadow single-agent retry must fire the same way swarm-parallel retries
// fire for writer agents: keyed on the agent's id, driven by
// `genome_turn_modified[agent_id]` + `genome_quality_deltas[agent_id]`,
// bounded by `genome_retry_counts[agent_id]`. The shadow pipeline doesn't
// change any of that — the main agent is just another writer from the
// retry mechanism's perspective.
#[test]
fn parse_abort_handles_plain_command() {
    use super::super::chat_input::{parse_abort_command, AbortScope};
    assert_eq!(parse_abort_command("/abort"), Some(AbortScope::Current));
    assert_eq!(parse_abort_command("@abort"), Some(AbortScope::Current));
    assert_eq!(parse_abort_command("/abort   "), Some(AbortScope::Current));
    // Leading whitespace tolerated.
    assert_eq!(parse_abort_command("   /abort"), Some(AbortScope::Current));
}

#[test]
fn parse_abort_handles_all_argument() {
    use super::super::chat_input::{parse_abort_command, AbortScope};
    assert_eq!(parse_abort_command("/abort all"), Some(AbortScope::All));
    assert_eq!(parse_abort_command("/abort ALL"), Some(AbortScope::All));
    assert_eq!(parse_abort_command("@abort All"), Some(AbortScope::All));
}

#[test]
fn parse_abort_handles_agent_id_argument() {
    use super::super::chat_input::{parse_abort_command, AbortScope};
    assert_eq!(
        parse_abort_command("/abort claude-haiku-4-5"),
        Some(AbortScope::Agent("claude-haiku-4-5".into()))
    );
    assert_eq!(
        parse_abort_command("@abort gpt-5.4#swarm-mis-001-clone-03"),
        Some(AbortScope::Agent("gpt-5.4#swarm-mis-001-clone-03".into()))
    );
}

#[test]
fn parse_abort_rejects_substring_matches() {
    use super::super::chat_input::parse_abort_command;
    // Substring traps: `/abortif`, `@abortion` must NOT be commands.
    assert_eq!(parse_abort_command("/abortif you can"), None);
    assert_eq!(parse_abort_command("@abortion"), None);
    // Unrelated prompts pass through.
    assert_eq!(parse_abort_command("hello"), None);
    assert_eq!(parse_abort_command(""), None);
}
