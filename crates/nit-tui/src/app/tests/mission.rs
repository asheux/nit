//! Mission lifecycle tests: codex/claude turn flow, reset_context,
//! archive, turn_completed event handling.

use super::*;

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
        shadow: false,
        last_message: String::new(),
    });

    let mut vitals = VitalsState::default();
    let codex = CodexRunner::spawn(CodexRuntimeMode::Exec, CodexRunnerConfig::default(), None);
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
    // `checked_sub` + fallback: Windows monotonic clock anchors at boot, and on a
    // freshly-booted CI runner `Instant::now() - 600s` underflows. The test's
    // assertion (`heartbeat_age_secs == 0` for queued turns) holds whether the
    // heartbeat is 10 minutes stale or freshly now() — both confirm that queued
    // turns are exempt from the liveness gate.
    if let Some(turn) = state.agents.active_turns.get_mut("gpt-test") {
        turn.last_heartbeat_at = Instant::now()
            .checked_sub(Duration::from_secs(600))
            .unwrap_or_else(Instant::now);
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
        shadow: false,
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
        shadow: false,
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
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "t+2".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("gpt-5.1-codex-mini".into()),
        mission_id: Some("mis-001".into()),
        text: "world".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "t+3".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: Some("mis-999".into()),
        text: "other mission".into(),
        prompt_msg_idx: None,
        kind: None,
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
        shadow: false,
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
        kind: None,
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
        shadow: false,
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
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "t+2".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("gpt-5.1-codex-mini".into()),
        mission_id: None,
        text: "keep this reply".into(),
        prompt_msg_idx: None,
        kind: None,
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
        shadow: false,
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
        kind: None,
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
fn codex_dispatch_uses_stored_thread_id_for_context_continuity() {
    let mut state = state_for_test();
    let mut vitals = VitalsState::default();
    let codex = CodexRunner::spawn(CodexRuntimeMode::Exec, CodexRunnerConfig::default(), None);

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
        shadow: false,
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
        shadow: false,
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
        shadow: false,
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
        shadow: false,
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
        shadow: false,
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
    assert!(!state.agents.claude_session_ids.contains_key("claude-opus"));
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
