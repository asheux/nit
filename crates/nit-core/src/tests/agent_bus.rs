use super::*;
use crate::state::{AgentLane, AgentLaneKind, AgentStatus, AgentTurnState, AppState};

fn test_state() -> AppState {
    let editor = crate::Buffer::from_str("editor", "", None);
    let notes = crate::Buffer::from_str("notes", "", None);
    AppState::new(std::path::PathBuf::from("."), editor, notes)
}

fn add_codex_agent(state: &mut AppState, id: &str) {
    state.agents.agents.push(AgentLane {
        id: id.into(),
        role: id.into(),
        lane: "Codex".into(),
        kind: AgentLaneKind::Codex,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.active_turns.insert(
        id.into(),
        AgentTurnState {
            started_at: std::time::Instant::now(),
            last_heartbeat_at: std::time::Instant::now(),
            last_output_at: std::time::Instant::now(),
            stage: None,
        },
    );
}

fn add_claude_agent(state: &mut AppState, id: &str) {
    state.agents.agents.push(AgentLane {
        id: id.into(),
        role: id.into(),
        lane: "Claude".into(),
        kind: AgentLaneKind::Claude,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.active_turns.insert(
        id.into(),
        AgentTurnState {
            started_at: std::time::Instant::now(),
            last_heartbeat_at: std::time::Instant::now(),
            last_output_at: std::time::Instant::now(),
            stage: None,
        },
    );
}

#[test]
fn token_count_routes_to_codex_maps_for_codex_agent() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");
    state
        .agents
        .codex_effective_context_window_tokens
        .insert("gpt-test".into(), 128_000);

    let token_count = AgentTokenCount {
        total_tokens: 50_000,
        context_window: 128_000,
    };
    apply_codex_token_count(&mut state, "gpt-test", None, &token_count);

    assert_eq!(
        state.agents.codex_used_tokens.get("gpt-test").copied(),
        Some(50_000)
    );
    assert!(state
        .agents
        .codex_context_remaining_pct
        .contains_key("gpt-test"));
    // Claude maps should be empty.
    assert!(state.agents.claude_used_tokens.get("gpt-test").is_none());
}

#[test]
fn token_count_routes_to_claude_maps_for_claude_agent() {
    let mut state = test_state();
    add_claude_agent(&mut state, "claude-opus");
    state
        .agents
        .claude_effective_context_window_tokens
        .insert("claude-opus".into(), 200_000);

    let token_count = AgentTokenCount {
        total_tokens: 15_000,
        context_window: 200_000,
    };
    apply_codex_token_count(&mut state, "claude-opus", None, &token_count);

    assert_eq!(
        state.agents.claude_used_tokens.get("claude-opus").copied(),
        Some(15_000)
    );
    assert!(state
        .agents
        .claude_context_remaining_pct
        .contains_key("claude-opus"));
    // Codex maps should be empty.
    assert!(state.agents.codex_used_tokens.get("claude-opus").is_none());
}

#[test]
fn token_count_mission_scoped_for_claude() {
    let mut state = test_state();
    add_claude_agent(&mut state, "claude-opus");
    state
        .agents
        .claude_effective_context_window_tokens
        .insert("claude-opus".into(), 200_000);

    let token_count = AgentTokenCount {
        total_tokens: 30_000,
        context_window: 200_000,
    };
    apply_codex_token_count(&mut state, "claude-opus", Some("mis-001"), &token_count);

    assert_eq!(
        state
            .agents
            .claude_mission_used_tokens
            .get("mis-001")
            .and_then(|m| m.get("claude-opus"))
            .copied(),
        Some(30_000),
    );
    assert!(state
        .agents
        .claude_mission_context_remaining_pct
        .get("mis-001")
        .and_then(|m| m.get("claude-opus"))
        .is_some());
    // Global maps should be empty.
    assert!(state.agents.claude_used_tokens.get("claude-opus").is_none());
}

#[test]
fn turn_completed_stores_thread_id_in_codex_maps() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");

    let event = AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: Some("thread-abc".into()),
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
        Some("thread-abc"),
    );
}

#[test]
fn turn_completed_stores_mission_scoped_thread_id() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");

    let event = AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: Some("mis-001".into()),
        thread_id: Some("thread-mission".into()),
        token_count: None,
        message: "Done.".into(),
    };
    event.apply(&mut state);

    assert_eq!(
        state
            .agents
            .codex_mission_thread_ids
            .get("mis-001")
            .and_then(|m| m.get("gpt-test"))
            .map(|s| s.as_str()),
        Some("thread-mission"),
    );
    // Global map should be empty.
    assert!(state.agents.codex_thread_ids.get("gpt-test").is_none());
}

#[test]
fn turn_failed_source_label_matches_backend() {
    let mut state = test_state();
    add_claude_agent(&mut state, "claude-opus");

    let event = AgentBusEvent::TurnFailed {
        agent_id: "claude-opus".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "rate limited".into(),
    };
    event.apply(&mut state);

    // The alert source should say "claude", not "codex".
    let alert = state.agents.alerts.last().unwrap();
    assert_eq!(alert.source, "claude");

    // The status bar message should say "Claude failed:".
    let status = state.status.as_deref().unwrap();
    assert!(status.starts_with("Claude failed:"), "got: {status}");
}

#[test]
fn file_write_populates_mission_accumulator_when_agent_has_mission() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");
    state
        .agents
        .agents
        .iter_mut()
        .find(|a| a.id == "gpt-test")
        .unwrap()
        .current_mission = Some("mis-001".into());

    AgentBusEvent::FileWrite {
        agent_id: "gpt-test".into(),
        path: std::path::PathBuf::from("src/a.rs"),
    }
    .apply(&mut state);

    assert!(state
        .genome_mission_modified
        .get("mis-001")
        .unwrap()
        .contains(&std::path::PathBuf::from("src/a.rs")));
}

#[test]
fn file_write_without_mission_skips_mission_accumulator() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");

    AgentBusEvent::FileWrite {
        agent_id: "gpt-test".into(),
        path: std::path::PathBuf::from("src/a.rs"),
    }
    .apply(&mut state);

    assert!(state.genome_mission_modified.is_empty());
    // Per-turn attribution still works.
    assert!(state
        .genome_turn_modified
        .get("gpt-test")
        .unwrap()
        .contains(&std::path::PathBuf::from("src/a.rs")));
}

#[test]
fn mission_accumulator_survives_turn_started_clearing_per_turn_set() {
    // Regression: when a swarm agent runs multiple sequential tasks, each
    // TurnStarted clears `genome_turn_modified[agent]`. Before the mission
    // accumulator existed, the swarm's genome review saw only files from the
    // last turn — every earlier turn's work was invisible.
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");
    state
        .agents
        .agents
        .iter_mut()
        .find(|a| a.id == "gpt-test")
        .unwrap()
        .current_mission = Some("mis-001".into());

    // Turn 1: write file a.
    AgentBusEvent::FileWrite {
        agent_id: "gpt-test".into(),
        path: std::path::PathBuf::from("a.rs"),
    }
    .apply(&mut state);

    // TurnStarted for turn 2 — this clears `genome_turn_modified[gpt-test]`.
    AgentBusEvent::TurnStarted {
        agent_id: "gpt-test".into(),
        mission_id: Some("mis-001".into()),
        resume_thread_id: None,
    }
    .apply(&mut state);

    // Turn 2: write file b.
    AgentBusEvent::FileWrite {
        agent_id: "gpt-test".into(),
        path: std::path::PathBuf::from("b.rs"),
    }
    .apply(&mut state);

    // Per-turn only has the current turn's file.
    let per_turn = state.genome_turn_modified.get("gpt-test").unwrap();
    assert!(!per_turn.contains(&std::path::PathBuf::from("a.rs")));
    assert!(per_turn.contains(&std::path::PathBuf::from("b.rs")));

    // Mission accumulator has both.
    let mission = state.genome_mission_modified.get("mis-001").unwrap();
    assert!(mission.contains(&std::path::PathBuf::from("a.rs")));
    assert!(mission.contains(&std::path::PathBuf::from("b.rs")));
}

#[test]
fn turn_completed_prompt_idx_checks_both_codex_and_claude_maps() {
    let mut state = test_state();
    add_claude_agent(&mut state, "claude-opus");

    // Store prompt index in the Claude map (as dispatch does for Claude agents).
    state
        .agents
        .claude_turn_prompt_idx
        .insert("claude-opus".into(), 42);

    // Push a user prompt message so parent linking can work.
    state.agents.messages.push(AgentMessage {
        at: "t+0".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "user prompt".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let event = AgentBusEvent::TurnCompleted {
        agent_id: "claude-opus".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "response".into(),
    };
    event.apply(&mut state);

    // The response message should be linked to prompt index 42.
    let response = state.agents.messages.last().unwrap();
    assert_eq!(response.prompt_msg_idx, Some(42));
    // The claude_turn_prompt_idx should be consumed (removed).
    assert!(state
        .agents
        .claude_turn_prompt_idx
        .get("claude-opus")
        .is_none());
}
