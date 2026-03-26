use super::chat_input::push_chat_message;
use super::*;
use crate::swarm::{is_agent_busy, SwarmSize};
use crate::widgets::{agent_console_view, agent_ops_view};
use nit_core::AgentBusEvent;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

// Test helper functions (moved from mod.rs)

fn handle_agent_station_key(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
) -> bool {
    let mut clipboard = None;
    handle_agent_station_key_with_clipboard(
        key,
        state,
        vitals,
        codex,
        claude,
        swarm,
        &mut clipboard,
    )
}

fn handle_mouse_event(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    fuzzy_runtime: &mut FuzzySearchRuntime,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    handle_mouse_event_with_swarm(
        &SwarmRuntime::default(),
        mouse,
        screen,
        state,
        fuzzy_runtime,
        input_state,
        clipboard,
        theme,
    )
}

fn map_agent_console_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    map_agent_console_mouse_with_swarm(&SwarmRuntime::default(), mouse, screen, state, clamp)
}

fn handle_mouse_down(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    handle_mouse_down_with_swarm(
        &SwarmRuntime::default(),
        mouse,
        screen,
        state,
        input_state,
        clipboard,
        theme,
    )
}

fn handle_mouse_drag(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    handle_mouse_drag_with_swarm(
        &SwarmRuntime::default(),
        mouse,
        screen,
        state,
        input_state,
        clipboard,
        theme,
    )
}

fn state_for_test() -> AppState {
    let editor = nit_core::Buffer::from_str("editor", "", None);
    let notes = nit_core::Buffer::from_str("notes", "", None);
    AppState::new(std::path::PathBuf::from("."), editor, notes)
}

fn state_for_test_in_workspace(label: &str) -> AppState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let workspace =
        std::env::temp_dir().join(format!("nit-app-{label}-{}-{nanos}", std::process::id()));
    let _ = fs::remove_dir_all(&workspace);
    fs::create_dir_all(&workspace).expect("create workspace");
    let editor = nit_core::Buffer::from_str("editor", "", None);
    let notes = nit_core::Buffer::from_str("notes", "", None);
    AppState::new(workspace, editor, notes)
}

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
fn agent_ops_space_does_not_toggle_pause() {
    let mut state = state_for_test();
    state.focus = PaneId::JobOutput;
    let mut input = InputState::new();
    let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty());
    let action = map_key_to_action(key, &state, &mut input);
    assert!(action.is_none());
}

#[test]
fn codex_like_event_stream_updates_agent_panes() {
    let mut state = state_for_test();

    // Simulate an external runtime (Codex/Claude/etc.) driving NIT via NDJSON events.
    let events = [
        r#"{"type":"mcp_status","status":{"state":"Connected","endpoint":"stdio://nit-agentd","latency_ms":7,"last_error":null}}"#,
        r#"{"type":"mission_upsert","mission":{"id":"mis-001","title":"Wire up Codex runtime","phase":"Execute","swarm":false,"assigned_agents":["codex"],"status":"RUNNING","updated_at":"t+1"}}"#,
        r#"{"type":"agent_upsert","agent":{"id":"codex","role":"Coder","lane":"Lane A","status":"Running","heartbeat_age_secs":1,"queue_len":1,"current_mission":"mis-001","last_message":"boot"}}"#,
        r#"{"type":"message_append","message":{"at":"t+2","channel":"Agent","agent_id":null,"mission_id":"mis-001","text":"Please integrate Codex."}}"#,
        r#"{"type":"message_append","message":{"at":"t+3","channel":"Agent","agent_id":"codex","mission_id":"mis-001","text":"Acknowledged. Streaming events into AgentsState now."}}"#,
        r#"{"type":"alert_append","alert":{"severity":"Warn","source":"codex","message":"This is a long alert message that should wrap into multiple lines in the Agent Ops Alerts table for smaller widths.","at":"t+4"}}"#,
    ];

    let start_epoch = state.agents.event_epoch;
    for json in events {
        let ev: AgentBusEvent = serde_json::from_str(json).expect("parse AgentBusEvent");
        ev.apply(&mut state);
    }
    assert!(state.agents.event_epoch > start_epoch);

    // Roster tab should show the Codex agent.
    state.agents.dock_tab = AgentOpsTab::Roster;
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Unknown);
    let roster = agent_ops_view::current_lines_for_width(&state, 72);
    assert!(roster.iter().any(|line| line.contains("Coder")));

    // Missions tab should show the mission + agent list in a vertical column.
    state.agents.dock_tab = AgentOpsTab::Missions;
    let missions = agent_ops_view::current_lines_for_width(&state, 72);
    assert!(missions.iter().any(|line| line.contains("mis-001")));

    // Alerts tab should wrap long messages and keep click mapping stable across wrapped rows.
    let alert_width = 48usize;
    state.agents.dock_tab = AgentOpsTab::Alerts;
    let alerts = agent_ops_view::current_lines_for_width(&state, alert_width);
    assert!(alerts.len() >= 5); // header + separator + at least two wrapped rows
    assert!(alerts[2].contains("WARN"));
    assert_eq!(
        agent_ops_view::alert_index_for_body_line(&state, alert_width, 0),
        Some(0)
    );
    assert_eq!(
        agent_ops_view::alert_index_for_body_line(&state, alert_width, 1),
        Some(0)
    );

    // Thread selection/export suppresses agent reply bodies so the transcript stays readable.
    let thread = agent_console_view::thread_lines_for_selection(&state, 80).join("\n");
    assert!(thread.contains("Please integrate Codex."));
    assert!(thread.contains("[Coder/codex]"));
    assert!(thread.contains("done (see ARTIFACTS)"));
    assert!(!thread.contains("Streaming events into AgentsState"));
}

#[test]
fn codex_dispatch_marks_turn_waiting_until_backend_starts() {
    let mut state = state_for_test();
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-test".into(),
        role: "Test".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

    let mut vitals = VitalsState::default();
    let codex = CodexRunner::spawn(CodexRuntimeMode::Exec, CodexRunnerConfig::default());
    // Don't shutdown: we only care about the immediate UI state set by the dispatch call.
    // The runner thread will receive the command but won't affect AppState before assertions run.

    maybe_dispatch_codex_turn(
        &mut state,
        &mut vitals,
        Some(&codex),
        Some("gpt-test".into()),
        Some("mis-001".into()),
        "hello".into(),
        true,
    );

    let agent = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id == "gpt-test")
        .expect("agent present");
    assert_eq!(agent.status, AgentStatus::Waiting);
    assert_eq!(agent.queue_len, 1);
    assert_eq!(agent.heartbeat_age_secs, 0);
    assert_eq!(agent.current_mission.as_deref(), Some("mis-001"));

    let turn = state
        .agents
        .active_turns
        .get("gpt-test")
        .expect("active turn inserted");
    assert_eq!(turn.stage.as_deref(), Some("queued"));

    // Liveness sampling should not treat queued turns as stalled (no heartbeats yet).
    if let Some(turn) = state.agents.active_turns.get_mut("gpt-test") {
        turn.last_heartbeat_at = Instant::now() - Duration::from_secs(600);
    }
    tick_agent_turn_liveness(&mut state);
    let agent = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id == "gpt-test")
        .expect("agent present");
    assert_eq!(agent.heartbeat_age_secs, 0);
}

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
fn codex_turn_completed_stores_mission_thread_id_and_marks_live() {
    let mut state = state_for_test();
    state.agents.missions.push(MissionRecord {
        id: "mis-001".into(),
        title: "Test mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["gpt-5.1-codex-mini".into()],
        status: "QUEUED".into(),
        updated_at: "t+0".into(),
    });
    state.agents.selected_mission = Some("mis-001".into());
    state.agents.mission_selected = 0;
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.1-codex-mini".into(),
        role: "gpt-5.1-codex-mini".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-5.1-codex-mini".into(),
        mission_id: Some("mis-001".into()),
        thread_id: Some("thread-123".into()),
        token_count: None,
        message: "ok".into(),
    }
    .apply(&mut state);

    assert_eq!(
        state
            .agents
            .codex_mission_thread_ids
            .get("mis-001")
            .and_then(|threads| threads.get("gpt-5.1-codex-mini"))
            .map(|s| s.as_str()),
        Some("thread-123")
    );
    assert_eq!(state.agents.missions[0].status.to_ascii_uppercase(), "LIVE");
    assert!(state
        .agents
        .messages
        .iter()
        .any(|msg| msg.mission_id.as_deref() == Some("mis-001")
            && msg.agent_id.as_deref() == Some("gpt-5.1-codex-mini")
            && msg.text == "ok"));
}

#[test]
fn reset_context_in_mission_forgets_codex_thread_id_and_clears_mission_thread() {
    let mut state = state_for_test();
    state.agents.missions.push(MissionRecord {
        id: "mis-001".into(),
        title: "Test mission".into(),
        phase: MissionPhase::Execute,
        swarm: true,
        assigned_agents: vec!["gpt-5.1-codex-mini".into(), "gpt-5.3-codex".into()],
        status: "LIVE".into(),
        updated_at: "t+0".into(),
    });
    state.agents.selected_mission = Some("mis-001".into());
    state.agents.mission_selected = 0;
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.1-codex-mini".into(),
        role: "gpt-5.1-codex-mini".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: Some("mis-001".into()),
        last_message: String::new(),
    });
    state.agents.selected_agent = Some("gpt-5.1-codex-mini".into());
    state.agents.roster_selected = state.agents.agents.len().saturating_sub(1);

    state
        .agents
        .codex_mission_thread_ids
        .entry("mis-001".into())
        .or_default()
        .insert("gpt-5.1-codex-mini".into(), "thread-123".into());
    state.agents.messages.push(AgentMessage {
        at: "t+1".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: Some("mis-001".into()),
        text: "hello".into(),
        prompt_msg_idx: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "t+2".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("gpt-5.1-codex-mini".into()),
        mission_id: Some("mis-001".into()),
        text: "world".into(),
        prompt_msg_idx: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "t+3".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: Some("mis-999".into()),
        text: "other mission".into(),
        prompt_msg_idx: None,
    });

    assert!(reset_roster_context(&mut state, &SwarmRuntime::default()));
    assert!(!state
        .agents
        .codex_mission_thread_ids
        .contains_key("mis-001"));
    assert!(state
        .agents
        .messages
        .iter()
        .all(|msg| msg.mission_id.as_deref() != Some("mis-001")));
    assert!(state
        .agents
        .messages
        .iter()
        .any(|msg| msg.mission_id.as_deref() == Some("mis-999")));
}

#[test]
fn reset_context_persists_mission_artifacts_before_clearing_live_thread() {
    let mut state = state_for_test_in_workspace("mission-persist");
    state.agents.missions.push(MissionRecord {
        id: "mis-401".into(),
        title: "Persist mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["gpt-5.1-codex-mini".into()],
        status: "LIVE".into(),
        updated_at: "t+0".into(),
    });
    state.agents.selected_mission = Some("mis-401".into());
    state.agents.mission_selected = 0;
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.1-codex-mini".into(),
        role: "gpt-5.1-codex-mini".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: Some("mis-401".into()),
        last_message: String::new(),
    });
    state.agents.selected_agent = Some("gpt-5.1-codex-mini".into());
    state.agents.roster_selected = 0;
    state.agents.messages.push(AgentMessage {
        at: "t+1".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: Some("mis-401".into()),
        text: "persist me".into(),
        prompt_msg_idx: None,
    });

    assert!(reset_roster_context(&mut state, &SwarmRuntime::default()));

    let run_path = state
        .workspace_root
        .join(".nit/agents/runs/mis-401/run.json");
    let run: serde_json::Value =
        serde_json::from_slice(&fs::read(&run_path).expect("read run")).expect("parse run");
    assert_eq!(run["messages"][0]["text"], "persist me");
}

#[test]
fn reset_context_persists_ad_hoc_artifacts_before_clearing_live_thread() {
    let mut state = state_for_test_in_workspace("adhoc-persist");
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.1-codex-mini".into(),
        role: "gpt-5.1-codex-mini".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("gpt-5.1-codex-mini".into());
    state.agents.roster_selected = state.agents.agents.len().saturating_sub(1);
    state.agents.messages.push(AgentMessage {
        at: "t+1".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "keep this prompt".into(),
        prompt_msg_idx: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "t+2".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("gpt-5.1-codex-mini".into()),
        mission_id: None,
        text: "keep this reply".into(),
        prompt_msg_idx: None,
    });
    state
        .agents
        .codex_thread_ids
        .insert("gpt-5.1-codex-mini".into(), "thread-adhoc".into());

    assert!(reset_roster_context(&mut state, &SwarmRuntime::default()));

    let run_path = state
        .workspace_root
        .join(".nit/agents/ad-hoc/gpt-5_1-codex-mini/run.json");
    let run: serde_json::Value =
        serde_json::from_slice(&fs::read(&run_path).expect("read run")).expect("parse run");
    assert_eq!(run["messages"][0]["text"], "keep this prompt");
    assert_eq!(run["messages"][1]["text"], "keep this reply");
    assert_eq!(run["codex_thread_id"], "thread-adhoc");
}

#[test]
fn reset_context_archives_saved_run_history_snapshot() {
    let mut state = state_for_test_in_workspace("mission-history-archive");
    state.agents.missions.push(MissionRecord {
        id: "mis-777".into(),
        title: "Archive mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["gpt-5.1-codex-mini".into()],
        status: "DONE".into(),
        updated_at: "t+0".into(),
    });
    state.agents.selected_mission = Some("mis-777".into());
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.1-codex-mini".into(),
        role: "gpt-5.1-codex-mini".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: Some("mis-777".into()),
        last_message: String::new(),
    });
    state.agents.selected_agent = Some("gpt-5.1-codex-mini".into());
    state.agents.roster_selected = 0;
    state.agents.messages.push(AgentMessage {
        at: "t+1".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: Some("mis-777".into()),
        text: "archive me".into(),
        prompt_msg_idx: None,
    });

    assert!(reset_roster_context(&mut state, &SwarmRuntime::default()));

    let history_root = state
        .workspace_root
        .join(".nit/agents/runs/mis-777/history");
    let mut history_entries = fs::read_dir(&history_root)
        .expect("history dir")
        .filter_map(|entry| entry.ok())
        .collect::<Vec<_>>();
    history_entries.sort_by_key(|entry| entry.file_name());
    assert_eq!(history_entries.len(), 1);
    let archived_run = history_entries[0].path().join("run.json");
    let run: serde_json::Value =
        serde_json::from_slice(&fs::read(&archived_run).expect("read archived run"))
            .expect("parse archived run");
    assert_eq!(run["messages"][0]["text"], "archive me");
}

#[test]
fn archive_saved_run_snapshot_prunes_old_history_entries() {
    let workspace = state_for_test_in_workspace("history-prune").workspace_root;
    let run_dir = workspace.join(".nit/agents/runs/mis-prune");
    fs::create_dir_all(&run_dir).expect("run dir");
    fs::write(
        run_dir.join("run.json"),
        serde_json::json!({
            "id": "mis-prune",
            "updated_at": "t+1",
            "messages": [],
            "patches": [],
            "evidence": []
        })
        .to_string(),
    )
    .expect("write run");

    let history_root = run_dir.join("history");
    fs::create_dir_all(&history_root).expect("history root");
    for idx in 0..=MAX_SAVED_RUN_HISTORY_PER_CONTEXT {
        let archive_dir = history_root.join(format!("{idx:020}"));
        fs::create_dir_all(&archive_dir).expect("archive dir");
        fs::write(
            archive_dir.join("run.json"),
            serde_json::json!({
                "id": "mis-prune",
                "updated_at": "t+0",
                "messages": [],
                "patches": [],
                "evidence": []
            })
            .to_string(),
        )
        .expect("write archived run");
    }

    archive_saved_run_snapshot(&run_dir).expect("archive snapshot");

    let archive_dirs = fs::read_dir(&history_root)
        .expect("read history")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .count();
    assert_eq!(archive_dirs, MAX_SAVED_RUN_HISTORY_PER_CONTEXT);
    assert!(!history_root.join(format!("{:020}", 0)).exists());
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
        .join(".nit/agents/runs/mis-888/history/00000000000000000009");
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
fn swarm_bulk_auto_switches_ops_to_dag() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.chat_input = "@swarm 2 template=bulk do thing".into();
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
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Dag);
}

#[test]
fn swarm_auto_detects_template_line_without_prefix() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

    state.agents.chat_input =
        "You are the SWARM PLANNER inside nit.\nTemplate: `parallel`\nDo thing.".into();
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
    assert!(state.agents.missions.iter().any(|mission| mission.swarm));
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("Swarm[parallel]")));
}

#[test]
fn swarm_auto_detects_swarm_role_and_uses_default_template() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.swarm_default_template = "bulk".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.chat_input = "You are the SWARM SYNTHESIZER.\nCombine agent outputs.".into();
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
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Dag);
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("Swarm[bulk]")));
}

#[test]
fn swarm_auto_detects_plain_prompt_when_bulk_template_selected() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.swarm_default_template = "bulk".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.chat_input = "do a quick repo health check and suggest next steps".into();
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
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Dag);
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("Swarm[bulk]")));
}

#[test]
fn swarm_autostart_uses_codex_max_parallel_turns_as_size_hint() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.swarm_default_template = "bulk".into();
    state.agents.codex_max_parallel_turns = 6;
    for idx in 0..6 {
        let id = if idx == 0 {
            "planner".to_string()
        } else {
            format!("worker-{idx}")
        };
        state.agents.agents.push(nit_core::AgentLane {
            id,
            role: "Codex".into(),
            lane: "codex".into(),
            kind: nit_core::AgentLaneKind::Codex,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
        });
    }
    state.agents.chat_input = "do a quick repo health check and suggest next steps".into();
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
    let assigned = state
        .agents
        .missions
        .first()
        .map(|mission| mission.assigned_agents.len())
        .unwrap_or(0);
    assert_eq!(assigned, 6);
}

#[test]
fn swarm_uses_roster_default_template_when_argument_missing() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.swarm_default_template = "parallel".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.chat_input = "@swarm 2 do thing".into();
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
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("Swarm[parallel]")));
}

#[test]
fn swarm_uses_roster_default_mission_when_argument_missing() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.swarm_default_template = "parallel".into();
    state.agents.swarm_default_mission = "research".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.chat_input = "@swarm 2 read papers and compare ideas".into();
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
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("(research)")));
}

#[test]
fn explicit_prompt_mission_overrides_roster_default_mission() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.swarm_default_template = "parallel".into();
    state.agents.swarm_default_mission = "general".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.chat_input = "@swarm 2 Mission: research\nread papers and compare ideas".into();
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
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("(research)")));
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
        last_message: "idle".into(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "hello world".into(),
        prompt_msg_idx: None,
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
        last_message: "idle".into(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "hello world".into(),
        prompt_msg_idx: None,
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
        last_message: String::new(),
    });

    let mut vitals = VitalsState::default();
    let swarm = SwarmRuntime::default();

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
        &swarm,
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
        &swarm,
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
        last_message: String::new(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: (0..80).map(|idx| format!("prompt-line-{idx}\n")).collect(),
        prompt_msg_idx: None,
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

    let mut vitals = VitalsState::default();
    let mut clipboard = None;
    // Ctrl+Up scrolls the content (plain Up navigates the input cursor).
    assert!(handle_artifacts_popup_key(
        &KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL),
        &mut state,
        &mut swarm,
        &mut vitals,
        None,
        None,
        &mut clipboard,
        screen,
        &theme,
    ));
    assert_eq!(
        state.agents.artifacts_popup_scroll,
        max_scroll.saturating_sub(1)
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
        message: "clone output".into(),
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
        message: "clone output".into(),
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
        message: "verify output".into(),
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
        message: "clone output".into(),
    };
    clone_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &clone_event);

    let verify_event = AgentBusEvent::TurnCompleted {
        agent_id: clone_id,
        mission_id: Some(mission_id.clone()),
        thread_id: Some("thr-verify".into()),
        token_count: None,
        message: "verify output".into(),
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
        message: "clone output".into(),
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
        &mut state, &swarm, 120, line_idx
    ));
    assert!(!state.agents.artifacts_popup_open);
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
        last_message: "idle".into(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "selection precision".into(),
        prompt_msg_idx: None,
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

    let mut runtime = nit_games::RuntimeAcceleratorStats::default();
    runtime.backend = nit_games::RuntimeAcceleratorBackend::Metal;
    runtime.metal_matches = 913_936;
    runtime.metal_matches_per_batch = Some(262_144);
    runtime.metal_inflight_batches = Some(5);
    runtime.metal_policy_cache_path = Some(
        "/Users/nitrika/Library/Caches/dev.openai.nit/games/metal-policy/apple_m4_max_1872106799188804901_v1.json"
            .into(),
    );
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
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "omega prompt".into(),
        prompt_msg_idx: None,
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
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "short".into(),
        prompt_msg_idx: None,
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
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "short".into(),
        prompt_msg_idx: None,
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
fn agent_console_mouse_drag_copies_selected_chat_text() {
    let mut state = state_for_test();
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "selection copy works".into(),
        prompt_msg_idx: None,
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
        last_message: "active".into(),
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "intro\n| table row |".into(),
        prompt_msg_idx: None,
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
fn codex_dispatch_uses_stored_thread_id_for_context_continuity() {
    let mut state = state_for_test();
    let mut vitals = VitalsState::default();
    let codex = CodexRunner::spawn(CodexRuntimeMode::Exec, CodexRunnerConfig::default());

    // Create a Codex agent.
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-test".into(),
        role: "Test".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state
        .agents
        .codex_effective_context_window_tokens
        .insert("gpt-test".into(), 128_000);

    // Simulate a completed turn that stores a thread_id.
    state
        .agents
        .codex_thread_ids
        .insert("gpt-test".into(), "thread-abc-123".into());

    // Dispatch a new turn — it should resume the stored thread.
    maybe_dispatch_codex_turn(
        &mut state,
        &mut vitals,
        Some(&codex),
        Some("gpt-test".into()),
        None,
        "follow-up question".into(),
        true,
    );

    // The agent should be queued/running.
    assert!(state.agents.active_turns.contains_key("gpt-test"));
}

#[test]
fn codex_mission_thread_ids_scoped_per_mission() {
    let mut state = state_for_test();

    // Simulate thread IDs stored for two different missions.
    state
        .agents
        .codex_mission_thread_ids
        .entry("mis-001".into())
        .or_default()
        .insert("gpt-test".into(), "thread-mission-1".into());
    state
        .agents
        .codex_mission_thread_ids
        .entry("mis-002".into())
        .or_default()
        .insert("gpt-test".into(), "thread-mission-2".into());

    // Mission-1 context retrieves thread for mission-1.
    let thread_1 = state
        .agents
        .codex_mission_thread_ids
        .get("mis-001")
        .and_then(|m| m.get("gpt-test"))
        .cloned();
    assert_eq!(thread_1.as_deref(), Some("thread-mission-1"));

    // Mission-2 context retrieves thread for mission-2.
    let thread_2 = state
        .agents
        .codex_mission_thread_ids
        .get("mis-002")
        .and_then(|m| m.get("gpt-test"))
        .cloned();
    assert_eq!(thread_2.as_deref(), Some("thread-mission-2"));
}

#[test]
fn claude_session_ids_scoped_per_mission() {
    let mut state = state_for_test();

    // Simulate session IDs stored for two different missions.
    state
        .agents
        .claude_mission_session_ids
        .entry("mis-001".into())
        .or_default()
        .insert("claude-opus".into(), "session-mission-1".into());
    state
        .agents
        .claude_mission_session_ids
        .entry("mis-002".into())
        .or_default()
        .insert("claude-opus".into(), "session-mission-2".into());

    // Mission-1 context retrieves session for mission-1.
    let session_1 = state
        .agents
        .claude_mission_session_ids
        .get("mis-001")
        .and_then(|m| m.get("claude-opus"))
        .cloned();
    assert_eq!(session_1.as_deref(), Some("session-mission-1"));

    // Mission-2 context retrieves session for mission-2.
    let session_2 = state
        .agents
        .claude_mission_session_ids
        .get("mis-002")
        .and_then(|m| m.get("claude-opus"))
        .cloned();
    assert_eq!(session_2.as_deref(), Some("session-mission-2"));
}

#[test]
fn claude_dispatch_uses_stored_session_id_for_context_continuity() {
    let mut state = state_for_test();
    let mut vitals = VitalsState::default();
    let claude = ClaudeRunner::spawn(ClaudeRunnerConfig::default());

    // Create a Claude agent.
    state.agents.agents.push(nit_core::AgentLane {
        id: "claude-opus".into(),
        role: "Opus".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state
        .agents
        .claude_effective_context_window_tokens
        .insert("claude-opus".into(), 200_000);
    state
        .agents
        .claude_supported_efforts
        .insert("claude-opus".into(), vec!["low".into(), "high".into()]);
    state
        .agents
        .claude_default_effort
        .insert("claude-opus".into(), "high".into());
    state
        .agents
        .claude_selected_effort
        .insert("claude-opus".into(), "high".into());

    // Simulate a completed turn that stores a session_id.
    state
        .agents
        .claude_session_ids
        .insert("claude-opus".into(), "session-xyz-789".into());

    // Dispatch a new turn — it should resume the stored session.
    maybe_dispatch_claude_turn(
        &mut state,
        &mut vitals,
        Some(&claude),
        Some("claude-opus".into()),
        None,
        "follow-up question".into(),
        true,
    );

    // The agent should be queued/running.
    assert!(state.agents.active_turns.contains_key("claude-opus"));
}

#[test]
fn turn_completed_stores_thread_id_for_codex() {
    let mut state = state_for_test();
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-test".into(),
        role: "Test".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.active_turns.insert(
        "gpt-test".into(),
        nit_core::state::AgentTurnState {
            started_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            last_output_at: Instant::now(),
            stage: None,
        },
    );

    // Simulate TurnCompleted from Codex with a thread_id (no mission).
    let event = AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: Some("thread-from-codex".into()),
        token_count: None,
        message: "Done.".into(),
    };
    event.apply(&mut state);

    assert_eq!(
        state
            .agents
            .codex_thread_ids
            .get("gpt-test")
            .map(|s| s.as_str()),
        Some("thread-from-codex"),
    );
}

#[test]
fn turn_completed_stores_session_id_for_claude_via_apply_claude_event() {
    let mut state = state_for_test();
    state.agents.agents.push(nit_core::AgentLane {
        id: "claude-opus".into(),
        role: "Opus".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.active_turns.insert(
        "claude-opus".into(),
        nit_core::state::AgentTurnState {
            started_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            last_output_at: Instant::now(),
            stage: None,
        },
    );

    // Simulate TurnCompleted from Claude with a session_id (no mission).
    let event = AgentBusEvent::TurnCompleted {
        agent_id: "claude-opus".into(),
        mission_id: None,
        thread_id: Some("session-from-claude".into()),
        token_count: None,
        message: "Done.".into(),
    };
    // Use apply_claude_event which stores in claude_session_ids.
    apply_claude_event(&mut state, &event);

    assert_eq!(
        state
            .agents
            .claude_session_ids
            .get("claude-opus")
            .map(|s| s.as_str()),
        Some("session-from-claude"),
    );
}

#[test]
fn turn_completed_stores_mission_scoped_session_for_claude() {
    let mut state = state_for_test();
    state.agents.agents.push(nit_core::AgentLane {
        id: "claude-opus".into(),
        role: "Opus".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: Some("mis-001".into()),
        last_message: String::new(),
    });
    state.agents.active_turns.insert(
        "claude-opus".into(),
        nit_core::state::AgentTurnState {
            started_at: Instant::now(),
            last_heartbeat_at: Instant::now(),
            last_output_at: Instant::now(),
            stage: None,
        },
    );

    let event = AgentBusEvent::TurnCompleted {
        agent_id: "claude-opus".into(),
        mission_id: Some("mis-001".into()),
        thread_id: Some("session-mis-001".into()),
        token_count: None,
        message: "Task done.".into(),
    };
    apply_claude_event(&mut state, &event);

    // Session should be stored under mission scope, not global.
    assert!(state.agents.claude_session_ids.get("claude-opus").is_none());
    assert_eq!(
        state
            .agents
            .claude_mission_session_ids
            .get("mis-001")
            .and_then(|m| m.get("claude-opus"))
            .map(|s| s.as_str()),
        Some("session-mis-001"),
    );
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
    let codex = CodexRunner::spawn(CodexRuntimeMode::Exec, CodexRunnerConfig::default());

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
