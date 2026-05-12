//! Multipane runtime tests: per-pane swarm completion drain and
//! queued-codex follow-up dispatch.

use super::*;

#[test]
fn multipane_runtime_drains_swarm_completion_into_completed_runs() {
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

    let mut vitals = VitalsState::default();
    let codex = CodexRunner::spawn(CodexRuntimeMode::Exec, CodexRunnerConfig::default(), None);
    let claude = ClaudeRunner::spawn(ClaudeRunnerConfig::default());
    let mut shadow = crate::shadow::ShadowRuntime::default();

    // Snapshot the active turns before the helper runs.
    let active_before = state.agents.active_turns.len();

    // Planner completes — drain helper must call swarm.handle_event_outcome
    // and dispatch the clone.
    let planner_event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: planner_message,
    };
    super::event_drain::drain_codex_event(
        &mut state,
        &mut vitals,
        &codex,
        &claude,
        &mut swarm,
        &mut shadow,
        None,
        planner_event,
    );

    // The drain helper must have advanced the swarm — the run should
    // have left Planning, and a clone task should be dispatched. The
    // bare-bus `event.apply` (the multipane regression) wouldn't have
    // touched any of this.
    let advanced = swarm.swarm_stage_label(&mission_id) != Some("PLAN");
    let active_after = state.agents.active_turns.len();
    assert!(
        advanced || active_after > active_before,
        "drain helper must advance the swarm past PLAN and dispatch the next stage; \
         stage={:?} active_before={} active_after={}",
        swarm.swarm_stage_label(&mission_id),
        active_before,
        active_after,
    );
}

/// Bug 4 fallback: when an event finishes (TurnCompleted), the helper
/// must fire `maybe_dispatch_next_queued_codex_turn` so a queued chat
/// turn moves out of `queued_codex_turns`. Multipane previously did
/// not call this — queue depth grew without bound.
#[test]
fn multipane_runtime_drains_queued_codex_turn_on_finish() {
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
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-test".into(),
        role: "Tester".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state
        .agents
        .queued_codex_turns
        .push_back(nit_core::QueuedCodexTurn {
            agent_id: "gpt-test".into(),
            mission_id: None,
            prompt: "queued".into(),
            prompt_msg_idx: None,
        });

    let mut vitals = VitalsState::default();
    let codex = CodexRunner::spawn(CodexRuntimeMode::Exec, CodexRunnerConfig::default(), None);
    let claude = ClaudeRunner::spawn(ClaudeRunnerConfig::default());
    let mut swarm = SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();

    let finish = AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "first turn done".into(),
    };
    let before = state.agents.queued_codex_turns.len();
    super::event_drain::drain_codex_event(
        &mut state,
        &mut vitals,
        &codex,
        &claude,
        &mut swarm,
        &mut shadow,
        None,
        finish,
    );
    let after = state.agents.queued_codex_turns.len();
    assert!(
        after < before,
        "queued turns must drain when the helper observes a finished event ({before} -> {after})",
    );
}

mod dispatch_resolve_cwd {
    use super::super::resolve_dispatch_cwd;
    use nit_core::{AppState, Buffer, MultipaneState, PaneSession};
    use std::path::PathBuf;

    fn fixture_state() -> AppState {
        AppState::new(
            PathBuf::from("/workspace"),
            Buffer::empty("scratch", None),
            Buffer::empty("notes", None),
        )
    }

    #[test]
    fn falls_back_to_workspace_root_when_not_multipane() {
        let state = fixture_state();
        assert_eq!(
            resolve_dispatch_cwd(&state, "any-agent"),
            PathBuf::from("/workspace")
        );
    }

    #[test]
    fn returns_pane_cwd_in_multipane() {
        let mut state = fixture_state();
        state.multipane = Some(MultipaneState {
            backend_agent_id: "claude-haiku-4-5".into(),
            panes: vec![
                PaneSession {
                    pane_id: 0,
                    agent_id: "claude-haiku-4-5#mp-pane-00".into(),
                    cwd: PathBuf::from("/pane0"),
                    ..PaneSession::default()
                },
                PaneSession {
                    pane_id: 1,
                    agent_id: "claude-haiku-4-5#mp-pane-01".into(),
                    cwd: PathBuf::from("/pane1"),
                    ..PaneSession::default()
                },
            ],
            focused: 0,
            grid_cols: 2,
            grid_rows: 1,
            backend_filter: Some("claude-haiku-4-5".into()),
            help_open: false,
        });
        assert_eq!(
            resolve_dispatch_cwd(&state, "claude-haiku-4-5#mp-pane-00"),
            PathBuf::from("/pane0")
        );
        assert_eq!(
            resolve_dispatch_cwd(&state, "claude-haiku-4-5#mp-pane-01"),
            PathBuf::from("/pane1")
        );
    }

    #[test]
    fn unknown_agent_falls_back() {
        let mut state = fixture_state();
        state.multipane = Some(MultipaneState {
            backend_agent_id: "claude-haiku-4-5".into(),
            panes: vec![PaneSession {
                pane_id: 0,
                agent_id: "claude-haiku-4-5#mp-pane-00".into(),
                cwd: PathBuf::from("/pane0"),
                ..PaneSession::default()
            }],
            focused: 0,
            grid_cols: 1,
            grid_rows: 1,
            backend_filter: Some("claude-haiku-4-5".into()),
            help_open: false,
        });
        assert_eq!(
            resolve_dispatch_cwd(&state, "non-pane-agent"),
            PathBuf::from("/workspace")
        );
    }

    // Lazy-bound pane: a roster selection commits both `agent_id` and
    // `selected_agent_id` to the pane-suffixed lane id, so dispatch routes
    // to the pane's cwd even before a backend is pinned.
    #[test]
    fn walks_lazy_no_backend_pane_lane() {
        let mut state = fixture_state();
        state.multipane = Some(MultipaneState {
            backend_agent_id: String::new(),
            panes: vec![PaneSession {
                pane_id: 0,
                agent_id: "claude-haiku-4-5#mp-pane-00".into(),
                cwd: PathBuf::from("/pane-lazy"),
                selected_agent_id: Some("claude-haiku-4-5#mp-pane-00".into()),
                ..PaneSession::default()
            }],
            focused: 0,
            grid_cols: 1,
            grid_rows: 1,
            backend_filter: None,
            help_open: false,
        });
        assert_eq!(
            resolve_dispatch_cwd(&state, "claude-haiku-4-5#mp-pane-00"),
            PathBuf::from("/pane-lazy")
        );
    }
}
