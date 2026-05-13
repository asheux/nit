//! Popup-rendering and key-routing tests: fuzzy / help / artifacts /
//! replay / strategy / global_archive / swarm_artifacts.

use super::*;

#[test]
fn fuzzy_popup_size_matches_preferred_when_tree_closed() {
    let state = state_for_test();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 200,
        height: 60,
    };
    let expected = fuzzy_search_popup::preferred_size(screen);
    assert_eq!(fuzzy_popup_size(screen, &state), expected);
}

#[test]
fn fuzzy_popup_size_matches_preferred_when_tree_open() {
    let mut state = state_for_test();
    state.file_tree.open = true;
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 200,
        height: 60,
    };
    let expected = fuzzy_search_popup::preferred_size(screen);
    assert_eq!(fuzzy_popup_size(screen, &state), expected);
}

#[test]
fn global_archive_popup_enter_selects_archived_run() {
    let mut state = state_for_test_in_workspace("history-popup-select");
    state.agents.dock_tab = AgentOpsTab::Evidence;
    state.agents.selected_mission = Some("mis-888".into());
    state.agents.missions.push(MissionRecord {
        id: "mis-888".into(),
        title: "History popup".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["gpt-5.1-codex-mini".into()],
        status: "DONE".into(),
        updated_at: "t+0".into(),
    });
    let run_dir = state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("runs")
        .join("mis-888")
        .join("history")
        .join("00000000000000000009");
    fs::create_dir_all(&run_dir).expect("history dir");
    let run_path = run_dir.join("run.json");
    fs::write(
        &run_path,
        serde_json::json!({
            "id": "mis-888",
            "updated_at": "t+5",
            "messages": [{"at":"t+1","channel":"Agent","agent_id":"gpt-5.1-codex-mini","mission_id":"mis-888","text":"saved reply"}],
            "patches": [],
            "evidence": []
        })
        .to_string(),
    )
    .expect("write run");

    // Build global archive index and open popup.
    state.agents.global_archive_open = true;
    state.agents.global_archive_index = agent_ops_view::build_global_archive_index(&state);
    state.agents.global_archive_filtered = agent_ops_view::filter_global_archive(
        &state.agents.global_archive_index,
        "",
        SavedRunHistoryFilter::All,
    );
    state.agents.global_archive_selected = 0;

    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };
    let theme = Theme::default();
    assert!(handle_global_archive_key(
        &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        screen,
        &theme,
    ));
    // RAG popup stays open behind the artifact detail window.
    assert!(state.agents.global_archive_open);
    assert!(state.agents.artifacts_popup_open);
    // The opened entry is stored for direct run.json loading; the Evidence
    // tab context (artifacts_selected_saved_run_path) is NOT changed.
    assert!(state.agents.global_archive_opened_entry.is_some());
    assert_eq!(
        state
            .agents
            .global_archive_opened_entry
            .as_ref()
            .unwrap()
            .run_path,
        run_path.to_string_lossy().as_ref()
    );
}

#[test]
fn global_archive_popup_filter_hotkeys_update_visible_scope() {
    let mut state = state_for_test_in_workspace("history-popup-filter-hotkeys");
    state.agents.selected_mission = Some("mis-889".into());
    state.agents.missions.push(MissionRecord {
        id: "mis-889".into(),
        title: "History filter keys".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["gpt-5.1-codex-mini".into()],
        status: "DONE".into(),
        updated_at: "t+0".into(),
    });
    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_micros();
    for archive_micros in [
        now_micros.saturating_sub(3 * 24 * 60 * 60 * 1_000_000),
        now_micros.saturating_sub(60 * 60 * 1_000_000),
    ] {
        let run_dir = state
            .workspace_root
            .join(".nit/agents/runs/mis-889/history")
            .join(format!("{archive_micros:020}"));
        fs::create_dir_all(&run_dir).expect("history dir");
        fs::write(
            run_dir.join("run.json"),
            serde_json::json!({
                "id": "mis-889",
                "updated_at": "t+1",
                "messages": [],
                "patches": [],
                "evidence": []
            })
            .to_string(),
        )
        .expect("write run");
    }

    // Build global archive index and open popup.
    state.agents.global_archive_open = true;
    state.agents.global_archive_index = agent_ops_view::build_global_archive_index(&state);
    state.agents.global_archive_filtered = agent_ops_view::filter_global_archive(
        &state.agents.global_archive_index,
        "",
        SavedRunHistoryFilter::All,
    );
    let all_count = state.agents.global_archive_filtered.len();

    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };
    let theme = Theme::default();
    // 'd' applies LastDay filter (query is empty so shortcut works).
    assert!(handle_global_archive_key(
        &KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        &mut state,
        screen,
        &theme,
    ));
    assert_eq!(
        state.agents.global_archive_filter,
        SavedRunHistoryFilter::LastDay
    );
    // After filtering to LastDay, fewer entries should be visible.
    assert!(state.agents.global_archive_filtered.len() <= all_count);
}

#[test]
fn global_archive_popup_fuzzy_search_filters_entries() {
    let mut state = state_for_test_in_workspace("history-popup-search");
    state.agents.missions.push(MissionRecord {
        id: "mis-890".into(),
        title: "Fix auth bug".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["codex-1".into()],
        status: "DONE".into(),
        updated_at: "t+0".into(),
    });
    let run_dir = state
        .workspace_root
        .join(".nit/agents/runs/mis-890/history/00000000000000000009");
    fs::create_dir_all(&run_dir).expect("history dir");
    fs::write(
        run_dir.join("run.json"),
        serde_json::json!({
            "id": "mis-890",
            "updated_at": "t+5",
            "messages": [
                {"at":"t+1","channel":"Agent","text":"Please fix the auth bug"},
                {"at":"t+2","channel":"Agent","agent_id":"codex-1","mission_id":"mis-890","text":"Fixed the authentication issue"}
            ],
            "patches": [],
            "evidence": []
        })
        .to_string(),
    )
    .expect("write run");

    // Build index.
    state.agents.global_archive_index = agent_ops_view::build_global_archive_index(&state);
    assert!(!state.agents.global_archive_index.is_empty());

    // No query: all entries.
    let all = agent_ops_view::filter_global_archive(
        &state.agents.global_archive_index,
        "",
        SavedRunHistoryFilter::All,
    );
    assert!(all.len() >= 2); // At least the 2 messages.

    // Fuzzy query "auth": should match.
    let filtered = agent_ops_view::filter_global_archive(
        &state.agents.global_archive_index,
        "auth",
        SavedRunHistoryFilter::All,
    );
    assert!(!filtered.is_empty());
    assert!(filtered.len() <= all.len());

    // Fuzzy query "zzzznonexistent": should match nothing.
    let empty = agent_ops_view::filter_global_archive(
        &state.agents.global_archive_index,
        "zzzznonexistent",
        SavedRunHistoryFilter::All,
    );
    assert!(empty.is_empty());
}

#[test]
fn global_archive_popup_esc_clears_query_then_closes() {
    let mut state = state_for_test_in_workspace("history-popup-esc");
    state.agents.global_archive_open = true;
    state.agents.global_archive_query = "test".into();

    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };
    let theme = Theme::default();
    // First Esc clears query.
    assert!(handle_global_archive_key(
        &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut state,
        screen,
        &theme,
    ));
    assert!(state.agents.global_archive_open); // Still open.
    assert!(state.agents.global_archive_query.is_empty());

    // Second Esc closes.
    assert!(handle_global_archive_key(
        &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut state,
        screen,
        &theme,
    ));
    assert!(!state.agents.global_archive_open);
}

#[test]
fn help_popup_scroll_clamps_before_moving_back_up() {
    let mut state = state_for_test();
    state.show_help = true;
    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 36,
    };
    let (max_scroll, _) = help_popup_scroll_metrics(screen, &theme);
    state.help_scroll = max_scroll.saturating_add(25);

    assert!(handle_help_popup_key(
        &KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        &mut state,
        screen,
        &theme,
    ));
    assert_eq!(state.help_scroll, max_scroll.saturating_sub(1));
}

#[test]
fn artifacts_popup_scroll_clamps_before_moving_back_up() {
    let mut state = state_for_test();
    state.agents.artifacts_popup_open = true;
    state.agents.selected_agent = Some("planner".into());
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Local".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: (0..80).map(|idx| format!("prompt-line-{idx}\n")).collect(),
        prompt_msg_idx: None,
        kind: None,
    });

    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 36,
    };
    let mut swarm = SwarmRuntime::default();
    let (max_scroll, _) = artifacts_popup_scroll_metrics(&state, &swarm, screen, &theme);
    state.agents.artifacts_popup_scroll = max_scroll.saturating_add(25);
    // Simulate what `artifacts_popup::render` does on each frame: cache the
    // current max_scroll and clamp the stored scroll. The scroll handlers now
    // rely on this cache (updated per render) instead of rebuilding the rendered
    // markdown on every keystroke.
    state.agents.artifacts_popup_last_max_scroll = max_scroll;
    state.agents.artifacts_popup_scroll = state.agents.artifacts_popup_scroll.min(max_scroll);

    let mut vitals = VitalsState::default();
    let mut clipboard = None;
    let mut shadow = crate::shadow::ShadowRuntime::default();
    // Ctrl+Up scrolls the content by the fast scroll step (plain Up navigates
    // the input cursor). Matches the editor's wheel step so keyboard nav feels
    // as fast as mouse-wheel scrolling.
    assert!(handle_artifacts_popup_key(
        &KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL),
        &mut state,
        &mut swarm,
        &mut shadow,
        &mut vitals,
        None,
        None,
        &mut clipboard,
        screen,
        &theme,
    ));
    assert_eq!(
        state.agents.artifacts_popup_scroll,
        max_scroll.saturating_sub(3)
    );
}

/// The forward-at-max clamp must hold on the very first wheel event even when
/// the cached `artifacts_popup_last_max_scroll` is still `usize::MAX` (i.e. no
/// render has run yet since the popup opened). Otherwise a burst of wheel-down
/// events would over-inflate the scroll offset past the real max, and a
/// subsequent reverse wheel would appear stuck until the inflation unwound.
#[test]
fn artifacts_popup_wheel_clamps_forward_at_max_and_allows_reverse() {
    let mut state = state_for_test();
    state.agents.artifacts_popup_open = true;
    state.agents.selected_agent = Some("planner".into());
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Local".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: (0..80).map(|idx| format!("prompt-line-{idx}\n")).collect(),
        prompt_msg_idx: None,
        kind: None,
    });

    // Cache is still the sentinel — simulates "no render has run yet since
    // popup open". The first wheel event must compute metrics inline and
    // clamp forward; reverse must work without being blocked by inflation.
    assert_eq!(
        state.agents.artifacts_popup_last_max_scroll,
        usize::MAX,
        "cache starts as sentinel",
    );

    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 36,
    };
    let swarm = SwarmRuntime::default();
    let (expected_max, _) = artifacts_popup_scroll_metrics(&state, &swarm, screen, &theme);
    assert!(
        expected_max > 0,
        "content must actually be scrollable for this test to be meaningful",
    );

    let area = dynamic_popup_rect(
        screen,
        crate::widgets::artifacts_popup::preferred_size(screen),
    );
    let wheel_down = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: area.x.saturating_add(2),
        row: area.y.saturating_add(2),
        modifiers: KeyModifiers::NONE,
    };
    let wheel_up = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: area.x.saturating_add(2),
        row: area.y.saturating_add(2),
        modifiers: KeyModifiers::NONE,
    };

    let mut fuzzy_runtime =
        FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
    let mut input_state = InputState::new();
    let mut clipboard = None;

    // Burst of wheel-down events well past the real max. The fallback path
    // (cache == usize::MAX) must compute metrics inline on the first event
    // and populate the cache, so subsequent events clamp at `expected_max`.
    for _ in 0..(expected_max + 25) {
        assert!(handle_mouse_event(
            wheel_down,
            screen,
            &mut state,
            &mut fuzzy_runtime,
            &mut input_state,
            &mut clipboard,
            &theme,
        ));
    }
    assert_eq!(
        state.agents.artifacts_popup_scroll, expected_max,
        "forward wheel past max must clamp at max_scroll, not inflate",
    );
    assert_eq!(
        state.agents.artifacts_popup_last_max_scroll, expected_max,
        "cache must be populated after first wheel event",
    );

    // Reverse wheel: must decrease from max immediately (not stuck).
    assert!(handle_mouse_event(
        wheel_up,
        screen,
        &mut state,
        &mut fuzzy_runtime,
        &mut input_state,
        &mut clipboard,
        &theme,
    ));
    assert!(
        state.agents.artifacts_popup_scroll < expected_max,
        "reverse wheel from max must actually decrease the scroll offset",
    );
}

#[test]
fn replay_popup_scroll_clamps_before_moving_back_up() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.games.replay.open = true;
    state.games.replay.lines = (0..80).map(|idx| format!("replay-line-{idx}")).collect();

    let theme = Theme::default();
    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 36,
    };
    let max_scroll = games_replay_popup_max_scroll(&state, screen, &theme);
    state.games.replay.scroll_offset = max_scroll.saturating_add(25);

    assert!(handle_replay_popup_key(
        &KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        &mut state,
        screen,
        &theme,
    ));
    assert_eq!(
        state.games.replay.scroll_offset,
        max_scroll.saturating_sub(1)
    );
}

#[test]
fn strategy_popup_scroll_clamps_before_moving_back_up() {
    let mut state = state_for_test();
    state.app_kind = AppKind::Games;
    state.games.strategy_inspect.open = true;
    state.games.strategy_inspect.lines =
        (0..80).map(|idx| format!("strategy-line-{idx}")).collect();

    let screen = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 36,
    };
    let max_scroll = games_strategy_popup_max_scroll(&state, screen);
    state.games.strategy_inspect.scroll_offset = max_scroll.saturating_add(25);

    assert!(handle_strategy_popup_key(
        &KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        &mut state,
        screen,
    ));
    assert_eq!(
        state.games.strategy_inspect.scroll_offset,
        max_scroll.saturating_sub(1)
    );
}

#[test]
fn swarm_artifacts_popup_follows_completed_clone_task() {
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
    state.agents.artifacts_popup_open = true;
    state.agents.artifacts_selected = 0;
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
    let planner_outcome = swarm.handle_event_outcome(&mut state, &planner_event);
    assert_eq!(planner_outcome.dispatches.len(), 1);
    assert!(matches!(
        agent_ops_view::artifacts_popup_ref(&state, &swarm, 120),
        Some(agent_ops_view::ArtifactsPopupRef::SwarmVerify { mission_id: ref mid })
            if mid == &mission_id
    ));

    let clone_event = AgentBusEvent::TurnCompleted {
        agent_id: clone_id,
        mission_id: Some(mission_id.clone()),
        thread_id: Some("thr-clone".into()),
        token_count: None,
        message: "clone output\n<SWARM_TASK_COMPLETE>".into(),
    };
    clone_event.apply(&mut state);
    let clone_outcome = swarm.handle_event_outcome(&mut state, &clone_event);
    maybe_follow_swarm_artifact_in_popup(&mut state, &swarm, clone_outcome.artifact_focus.as_ref());

    assert!(matches!(
        agent_ops_view::artifacts_popup_ref(&state, &swarm, 120),
        Some(agent_ops_view::ArtifactsPopupRef::SwarmTask {
            mission_id: ref mid,
            ref task_id,
        }) if mid == &mission_id && task_id == "t1"
    ));
    assert_eq!(state.agents.artifacts_popup_scroll, 0);
}

#[test]
fn swarm_artifacts_popup_follows_final_report() {
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
    state.agents.artifacts_popup_open = true;
    state.agents.artifacts_selected = 0;
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
    let clone_outcome = swarm.handle_event_outcome(&mut state, &clone_event);
    maybe_follow_swarm_artifact_in_popup(&mut state, &swarm, clone_outcome.artifact_focus.as_ref());

    assert!(matches!(
        agent_ops_view::artifacts_popup_ref(&state, &swarm, 120),
        Some(agent_ops_view::ArtifactsPopupRef::SwarmTask {
            mission_id: ref mid,
            ref task_id,
        }) if mid == &mission_id && task_id == "t1"
    ));

    let verify_event = AgentBusEvent::TurnCompleted {
        agent_id: clone_id,
        mission_id: Some(mission_id.clone()),
        thread_id: Some("thr-verify".into()),
        token_count: None,
        message: "verify output\n<SWARM_TASK_COMPLETE>".into(),
    };
    verify_event.apply(&mut state);
    let verify_outcome = swarm.handle_event_outcome(&mut state, &verify_event);
    assert_eq!(verify_outcome.dispatches.len(), 1);
    assert_eq!(verify_outcome.dispatches[0].agent_id, "planner");

    let report_event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: "# Final Report\n\nShip it.\n".into(),
    };
    report_event.apply(&mut state);
    let report_outcome = swarm.handle_event_outcome(&mut state, &report_event);
    maybe_follow_swarm_artifact_in_popup(
        &mut state,
        &swarm,
        report_outcome.artifact_focus.as_ref(),
    );

    assert!(matches!(
        report_outcome.artifact_focus,
        Some(SwarmArtifactFocus::Report { mission_id: ref mid }) if mid == &mission_id
    ));
    assert!(matches!(
        agent_ops_view::artifacts_popup_ref(&state, &swarm, 120),
        Some(agent_ops_view::ArtifactsPopupRef::SwarmReport { mission_id: ref mid })
            if mid == &mission_id
    ));
    assert_eq!(state.agents.artifacts_popup_scroll, 0);
}
