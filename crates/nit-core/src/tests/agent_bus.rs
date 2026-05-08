//! Tests for `AgentBusEvent::apply` — token accounting, turn lifecycle,
//! soft-cancel routing, file-write side effects, and substrate emission.

use super::*;
use std::path::PathBuf;

use crate::state::{AgentAlertSeverity, AgentChannel, AgentStatus, MissionPhase, MissionRecord};
use crate::test_helpers::{add_claude_agent, add_codex_agent, temp_dir, test_state};

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
    assert!(!state.agents.claude_used_tokens.contains_key("gpt-test"));
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
    assert!(!state.agents.codex_used_tokens.contains_key("claude-opus"));
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
    assert!(!state.agents.claude_used_tokens.contains_key("claude-opus"));
}

#[test]
fn turn_completed_stores_thread_id_in_codex_maps() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: Some("thread-abc".into()),
        token_count: None,
        message: "Done.".into(),
    }
    .apply(&mut state);

    assert_eq!(
        state
            .agents
            .codex_thread_ids
            .get("gpt-test")
            .map(String::as_str),
        Some("thread-abc"),
    );
}

#[test]
fn turn_completed_stores_mission_scoped_thread_id() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: Some("mis-001".into()),
        thread_id: Some("thread-mission".into()),
        token_count: None,
        message: "Done.".into(),
    }
    .apply(&mut state);

    assert_eq!(
        state
            .agents
            .codex_mission_thread_ids
            .get("mis-001")
            .and_then(|m| m.get("gpt-test"))
            .map(String::as_str),
        Some("thread-mission"),
    );
    assert!(!state.agents.codex_thread_ids.contains_key("gpt-test"));
}

#[test]
fn turn_failed_source_label_matches_backend() {
    let mut state = test_state();
    add_claude_agent(&mut state, "claude-opus");

    AgentBusEvent::TurnFailed {
        agent_id: "claude-opus".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "rate limited".into(),
    }
    .apply(&mut state);

    let alert = state.agents.alerts.last().unwrap();
    assert_eq!(alert.source, "claude");
    let status = state.status.as_deref().unwrap();
    assert!(status.starts_with("Claude failed:"), "got: {status}");
}

#[test]
fn turn_failed_with_operator_cancel_routes_soft_path() {
    // Pins the OPERATOR_CANCEL_TURN_MESSAGE soft-cancel branch in the bus.
    // Drift on either side (the const, the runner emit sites, or the bus
    // match) would silently flip operator cancels into the hard error path:
    // red status banner, substrate Warning, mission ERROR overwrite, alert
    // noise. This test fails loudly if any of those drift.
    let mut state = test_state();
    add_claude_agent(&mut state, "claude-opus");
    state.agents.missions.push(MissionRecord {
        id: "mis-001".into(),
        title: "test".into(),
        phase: MissionPhase::Execute,
        swarm: true,
        assigned_agents: vec!["claude-opus".into()],
        status: "ABORTED".into(),
        updated_at: String::new(),
    });
    let alerts_before = state.agents.alerts.len();
    let signals_before = state.substrate.signals.len();

    AgentBusEvent::TurnFailed {
        agent_id: "claude-opus".into(),
        mission_id: Some("mis-001".into()),
        thread_id: None,
        token_count: None,
        message: OPERATOR_CANCEL_TURN_MESSAGE.into(),
    }
    .apply(&mut state);

    let agent = state
        .agents
        .agents
        .iter()
        .find(|a| a.id == "claude-opus")
        .expect("claude-opus still in roster");
    assert_eq!(agent.status, AgentStatus::Idle);

    assert_eq!(
        state.agents.alerts.len(),
        alerts_before,
        "soft-cancel must not push an alert",
    );
    assert!(
        state.status.is_none(),
        "soft-cancel must not set the failure status banner, got: {:?}",
        state.status,
    );

    let new_warnings = state
        .substrate
        .signals
        .values()
        .filter(|s| s.kind == crate::substrate::SignalKind::Warning)
        .count();
    assert_eq!(
        state.substrate.signals.len() - signals_before,
        0,
        "soft-cancel must not emit substrate signals; got {new_warnings} new warnings",
    );

    let mission = state
        .agents
        .missions
        .iter()
        .find(|m| m.id == "mis-001")
        .unwrap();
    assert_eq!(
        mission.status, "ABORTED",
        "soft-cancel must not clobber an ABORTED mission status with ERROR",
    );

    let diag = state
        .agents
        .diag_events
        .last()
        .expect("soft-cancel must record an Info diag entry");
    assert_eq!(diag.severity, AgentAlertSeverity::Info);
    assert!(
        diag.message.contains("cancelled by operator"),
        "diag message should reflect operator cancel, got: {}",
        diag.message,
    );
}

#[test]
fn turn_failed_with_runner_internal_cancel_routes_soft_path() {
    // MCP-driven runner cancels (`Cancelled (MCP stop)` / `Cancelled (MCP
    // reconnect)`) ride the same soft path so reconfiguring MCP doesn't
    // surface as an alert.
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");
    let signals_before = state.substrate.signals.len();

    AgentBusEvent::TurnFailed {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "Cancelled (MCP reconnect)".into(),
    }
    .apply(&mut state);

    assert!(state.agents.alerts.is_empty());
    assert!(state.status.is_none());
    assert_eq!(state.substrate.signals.len(), signals_before);
    let agent = state
        .agents
        .agents
        .iter()
        .find(|a| a.id == "gpt-test")
        .unwrap();
    assert_eq!(agent.status, AgentStatus::Idle);
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
        mission_id: None,
        path: PathBuf::from("src/a.rs"),
    }
    .apply(&mut state);

    assert!(state
        .genome_mission_modified
        .get("mis-001")
        .unwrap()
        .contains(&PathBuf::from("src/a.rs")));
}

#[test]
fn file_write_with_explicit_mission_id_wins_over_agent_lookup() {
    // New emitters carry mission_id directly in the event so the mission
    // accumulator does not race with `TurnStarted` setting
    // `agent.current_mission`.
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");
    // Agent has no current_mission set — the lookup path would fail.
    AgentBusEvent::FileWrite {
        agent_id: "gpt-test".into(),
        mission_id: Some("mis-042".into()),
        path: PathBuf::from("src/x.rs"),
    }
    .apply(&mut state);

    assert!(state
        .genome_mission_modified
        .get("mis-042")
        .unwrap()
        .contains(&PathBuf::from("src/x.rs")));
}

#[test]
fn file_write_without_mission_skips_mission_accumulator() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");

    AgentBusEvent::FileWrite {
        agent_id: "gpt-test".into(),
        mission_id: None,
        path: PathBuf::from("src/a.rs"),
    }
    .apply(&mut state);

    assert!(state.genome_mission_modified.is_empty());
    assert!(state
        .genome_turn_modified
        .get("gpt-test")
        .unwrap()
        .contains(&PathBuf::from("src/a.rs")));
}

#[test]
fn mission_accumulator_survives_turn_started_clearing_per_turn_set() {
    // Regression: each TurnStarted clears `genome_turn_modified[agent]`.
    // Before the mission accumulator existed, the swarm's genome review saw
    // only files from the last turn — every earlier turn's work was invisible.
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
        mission_id: Some("mis-001".into()),
        path: PathBuf::from("a.rs"),
    }
    .apply(&mut state);

    AgentBusEvent::TurnStarted {
        agent_id: "gpt-test".into(),
        mission_id: Some("mis-001".into()),
        resume_thread_id: None,
    }
    .apply(&mut state);

    AgentBusEvent::FileWrite {
        agent_id: "gpt-test".into(),
        mission_id: Some("mis-001".into()),
        path: PathBuf::from("b.rs"),
    }
    .apply(&mut state);

    let per_turn = state.genome_turn_modified.get("gpt-test").unwrap();
    assert!(!per_turn.contains(&PathBuf::from("a.rs")));
    assert!(per_turn.contains(&PathBuf::from("b.rs")));

    let mission = state.genome_mission_modified.get("mis-001").unwrap();
    assert!(mission.contains(&PathBuf::from("a.rs")));
    assert!(mission.contains(&PathBuf::from("b.rs")));
}

#[test]
fn turn_completed_prompt_idx_checks_both_codex_and_claude_maps() {
    let mut state = test_state();
    add_claude_agent(&mut state, "claude-opus");

    state
        .agents
        .claude_turn_prompt_idx
        .insert("claude-opus".into(), 42);

    state.agents.messages.push(AgentMessage {
        at: "t+0".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "user prompt".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    AgentBusEvent::TurnCompleted {
        agent_id: "claude-opus".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "response".into(),
    }
    .apply(&mut state);

    let response = state.agents.messages.last().unwrap();
    assert_eq!(response.prompt_msg_idx, Some(42));
    assert!(!state
        .agents
        .claude_turn_prompt_idx
        .contains_key("claude-opus"));
}

#[test]
fn emit_signal_event_round_trip_and_apply() {
    let signal = crate::substrate::Signal {
        id: "1-agent-a-0".into(),
        kind: crate::substrate::SignalKind::Warning,
        posted_by: "agent-a".into(),
        posted_at_gen: 0,
        target: crate::substrate::SignalTarget::Global,
        initial_strength: 1.0,
        payload: serde_json::Value::Null,
    };
    let event = AgentBusEvent::EmitSignal {
        signal: signal.clone(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let restored: AgentBusEvent = serde_json::from_str(&json).unwrap();

    let mut state = test_state();
    restored.apply(&mut state);

    assert_eq!(state.substrate.signals.len(), 1);
    assert!(state.substrate.signals.contains_key(&signal.id));
}

#[test]
fn turn_completed_advances_substrate_generation() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");

    // Pre-seed a low-strength HelpNeeded that decays past the prune
    // threshold once TurnCompleted advances the generation. HelpNeeded's
    // decay_rate=0.5: at gen=1 the effective strength is 0.025 < 0.05.
    state.substrate.emit_signal(crate::substrate::Signal {
        id: "seed".into(),
        kind: crate::substrate::SignalKind::HelpNeeded,
        posted_by: "seed".into(),
        posted_at_gen: 0,
        target: crate::substrate::SignalTarget::Global,
        initial_strength: 0.05,
        payload: serde_json::Value::Null,
    });
    assert_eq!(state.substrate.current_generation(), 0);
    assert_eq!(state.substrate.signals.len(), 1);

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "ok".into(),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.current_generation(), 1);
    // Weak pre-seeded signal pruned; auto-emitted DoneMarker
    // (posted_at_gen=0, effective=0.95) survives.
    assert_eq!(state.substrate.signals.len(), 1);
    let remaining = state.substrate.signals.values().next().unwrap();
    assert_eq!(remaining.kind, crate::substrate::SignalKind::DoneMarker);
    assert_eq!(remaining.posted_by, "gpt-test");
}

#[test]
fn turn_completed_persists_substrate_to_disk() {
    let dir = temp_dir("apply-persist");
    let editor = crate::Buffer::from_str("editor", "", None);
    let notes = crate::Buffer::from_str("notes", "", None);
    let mut state = AppState::new(dir.clone(), editor, notes);
    add_codex_agent(&mut state, "gpt-test");

    AgentBusEvent::EmitSignal {
        signal: crate::substrate::Signal {
            id: "0-agent-a-0".into(),
            kind: crate::substrate::SignalKind::DoneMarker,
            posted_by: "agent-a".into(),
            posted_at_gen: 0,
            target: crate::substrate::SignalTarget::Global,
            initial_strength: 1.0,
            payload: serde_json::Value::Null,
        },
    }
    .apply(&mut state);

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "ok".into(),
    }
    .apply(&mut state);

    let path = dir.join(".nit").join("substrate").join("state.json");
    assert!(path.exists(), "substrate state.json should be written");

    let reloaded = crate::substrate::SubstrateState::load(&dir);
    assert_eq!(reloaded.current_generation(), 1);
    // Both the manually-emitted and auto-emitted DoneMarker survive at gen=1.
    assert_eq!(reloaded.signals.len(), 2);
    assert!(reloaded.signals.contains_key("0-agent-a-0"));
    let auto_emitted: Vec<_> = reloaded
        .signals
        .values()
        .filter(|s| s.posted_by == "gpt-test")
        .collect();
    assert_eq!(auto_emitted.len(), 1);
    assert_eq!(
        auto_emitted[0].kind,
        crate::substrate::SignalKind::DoneMarker,
    );
}

#[test]
fn turn_completed_emits_done_marker_signal() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");
    assert_eq!(state.substrate.signals.len(), 0);

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: Some("mis-001".into()),
        thread_id: Some("thread-xyz".into()),
        token_count: None,
        message: "Done.".into(),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.signals.len(), 1);
    let signal = state.substrate.signals.values().next().unwrap();
    assert_eq!(signal.kind, crate::substrate::SignalKind::DoneMarker);
    assert_eq!(signal.posted_by, "gpt-test");
    // Posted at the pre-advance generation (0); the counter then advances to 1.
    assert_eq!(signal.posted_at_gen, 0);
    match &signal.target {
        crate::substrate::SignalTarget::Agent { agent_id } => {
            assert_eq!(agent_id, "gpt-test");
        }
        other => panic!("expected Agent target, got {other:?}"),
    }
    assert_eq!(
        signal.payload.get("message").and_then(|v| v.as_str()),
        Some("Done."),
    );
    assert_eq!(state.substrate.current_generation(), 1);
}

#[test]
fn turn_failed_emits_warning_signal() {
    let mut state = test_state();
    add_claude_agent(&mut state, "claude-opus");
    let before_gen = state.substrate.current_generation();

    AgentBusEvent::TurnFailed {
        agent_id: "claude-opus".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "rate limited".into(),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.signals.len(), 1);
    let signal = state.substrate.signals.values().next().unwrap();
    assert_eq!(signal.kind, crate::substrate::SignalKind::Warning);
    assert_eq!(signal.posted_by, "claude-opus");
    match &signal.target {
        crate::substrate::SignalTarget::Agent { agent_id } => {
            assert_eq!(agent_id, "claude-opus");
        }
        other => panic!("expected Agent target, got {other:?}"),
    }
    assert_eq!(
        signal.payload.get("message").and_then(|v| v.as_str()),
        Some("rate limited"),
    );
    // TurnFailed does NOT advance the generation.
    assert_eq!(state.substrate.current_generation(), before_gen);
}

#[test]
fn file_write_auto_claim_inserts_exclusive_write() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");
    assert_eq!(state.substrate.claims.len(), 0);

    AgentBusEvent::FileWrite {
        agent_id: "gpt-test".into(),
        mission_id: None,
        path: PathBuf::from("src/a.rs"),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.claims.len(), 1);
    let claim = state.substrate.claims.values().next().unwrap();
    assert_eq!(claim.kind, crate::substrate::ClaimKind::ExclusiveWrite);
    match &claim.target {
        crate::substrate::ClaimTarget::File { path } => {
            assert_eq!(path, &PathBuf::from("src/a.rs"));
        }
        other => panic!("expected File target, got {other:?}"),
    }
    assert_eq!(claim.claimed_by, "gpt-test");
}

#[test]
fn file_write_auto_claim_conflict_emits_violation_and_queues_retry() {
    let mut state = test_state();
    add_codex_agent(&mut state, "a1");
    add_codex_agent(&mut state, "a2");
    let path = PathBuf::from("src/a.rs");

    let seeded = crate::substrate::Claim {
        id: "seed-c".into(),
        kind: crate::substrate::ClaimKind::ExclusiveWrite,
        target: crate::substrate::ClaimTarget::File { path: path.clone() },
        claimed_by: "a2".into(),
        claimed_at_gen: state.substrate.current_generation(),
        ttl_gens: 5,
        rationale: "seeded by test".into(),
    };
    state.substrate.assert_claim(seeded).unwrap();
    let claims_before = state.substrate.claims.len();
    let signals_before = state.substrate.signals.len();

    AgentBusEvent::FileWrite {
        agent_id: "a1".into(),
        mission_id: None,
        path: path.clone(),
    }
    .apply(&mut state);

    // (i) The conflicting writer's claim was NOT inserted.
    assert_eq!(state.substrate.claims.len(), claims_before);

    // (ii) Exactly one new ClaimViolation, posted by a1.
    let new_signals = state.substrate.signals.len() - signals_before;
    assert_eq!(new_signals, 1);
    let violation = state
        .substrate
        .signals
        .values()
        .find(|s| s.kind == crate::substrate::SignalKind::ClaimViolation)
        .expect("expected a ClaimViolation signal");
    assert_eq!(violation.posted_by, "a1");

    // (iii) Retry queued with agent_id=a1, conflicting_holder=a2.
    assert_eq!(state.pending_claim_retries.len(), 1);
    let req = &state.pending_claim_retries[0];
    assert_eq!(req.agent_id, "a1");
    assert_eq!(req.conflicting_holder, "a2");
    assert_eq!(req.path, path);
}

#[test]
fn turn_completed_expires_stale_claims() {
    let mut state = test_state();
    add_codex_agent(&mut state, "gpt-test");
    // Claim with TTL=1 at gen=0. After TurnCompleted advances gen to 1,
    // expire_claims drops it.
    let claim = crate::substrate::Claim {
        id: "stale".into(),
        kind: crate::substrate::ClaimKind::ExclusiveWrite,
        target: crate::substrate::ClaimTarget::File {
            path: PathBuf::from("a.rs"),
        },
        claimed_by: "seed".into(),
        claimed_at_gen: 0,
        ttl_gens: 1,
        rationale: "stale".into(),
    };
    state.substrate.assert_claim(claim).unwrap();
    assert_eq!(state.substrate.claims.len(), 1);

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "ok".into(),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.current_generation(), 1);
    assert!(state.substrate.claims.is_empty());
}

#[test]
fn file_write_invalidates_overlapping_assumption_and_emits_warning() {
    let mut state = test_state();
    add_codex_agent(&mut state, "writer-b");
    let path = PathBuf::from("src/a.rs");

    let assumption = crate::substrate::Assumption {
        id: "assm-1".into(),
        target: crate::substrate::AssumptionTarget::File { path: path.clone() },
        fact: serde_json::json!({"depends_on": "a.rs exists"}),
        posted_by: "poster-a".into(),
        posted_at_gen: state.substrate.current_generation(),
        ttl_gens: 10,
        rationale: "read-dependency".into(),
    };
    state.substrate.assert_assumption(assumption);
    assert_eq!(state.substrate.assumptions.len(), 1);
    let signals_before = state.substrate.signals.len();

    AgentBusEvent::FileWrite {
        agent_id: "writer-b".into(),
        mission_id: None,
        path: path.clone(),
    }
    .apply(&mut state);

    // (i) Assumption gone from substrate.
    assert!(
        state.substrate.assumptions.is_empty(),
        "assumption should be invalidated by overlapping write",
    );

    // (ii) Warning emitted by writer-b targeting Agent{poster-a}.
    let new_signal_count = state.substrate.signals.len() - signals_before;
    assert!(
        new_signal_count >= 1,
        "expected at least one new signal, got {new_signal_count}",
    );
    let warning = state
        .substrate
        .signals
        .values()
        .find(|s| {
            s.kind == crate::substrate::SignalKind::Warning
                && s.payload.get("reason").and_then(|v| v.as_str())
                    == Some("assumption_invalidated_by_write")
        })
        .expect("expected an invalidation Warning signal");
    assert_eq!(warning.posted_by, "writer-b");
    match &warning.target {
        crate::substrate::SignalTarget::Agent { agent_id } => {
            assert_eq!(agent_id, "poster-a");
        }
        other => panic!("expected Agent target, got {other:?}"),
    }
    assert_eq!(
        warning.payload.get("writer").and_then(|v| v.as_str()),
        Some("writer-b"),
    );
}

#[test]
fn file_write_leaves_non_overlapping_assumption_intact() {
    let mut state = test_state();
    add_codex_agent(&mut state, "writer-b");

    let assumption = crate::substrate::Assumption {
        id: "assm-a".into(),
        target: crate::substrate::AssumptionTarget::File {
            path: PathBuf::from("a.rs"),
        },
        fact: serde_json::json!({}),
        posted_by: "poster-a".into(),
        posted_at_gen: state.substrate.current_generation(),
        ttl_gens: 10,
        rationale: "read".into(),
    };
    state.substrate.assert_assumption(assumption);

    // Write to b.rs, not a.rs.
    AgentBusEvent::FileWrite {
        agent_id: "writer-b".into(),
        mission_id: None,
        path: PathBuf::from("b.rs"),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.assumptions.len(), 1);
    assert!(state.substrate.assumptions.contains_key("assm-a"));

    let invalidation_warnings = state
        .substrate
        .signals
        .values()
        .filter(|s| {
            s.payload.get("reason").and_then(|v| v.as_str())
                == Some("assumption_invalidated_by_write")
        })
        .count();
    assert_eq!(invalidation_warnings, 0);
}

#[test]
fn file_write_invalidates_assumption_even_when_auto_claim_conflicts() {
    let mut state = test_state();
    add_codex_agent(&mut state, "writer-c");
    let path = PathBuf::from("src/contested.rs");

    let pre_claim = crate::substrate::Claim {
        id: "pre-claim".into(),
        kind: crate::substrate::ClaimKind::ExclusiveWrite,
        target: crate::substrate::ClaimTarget::File { path: path.clone() },
        claimed_by: "other-a".into(),
        claimed_at_gen: state.substrate.current_generation(),
        ttl_gens: 50,
        rationale: "holds lock".into(),
    };
    state.substrate.assert_claim(pre_claim).unwrap();

    let assumption = crate::substrate::Assumption {
        id: "assm-b".into(),
        target: crate::substrate::AssumptionTarget::File { path: path.clone() },
        fact: serde_json::json!({}),
        posted_by: "poster-b".into(),
        posted_at_gen: state.substrate.current_generation(),
        ttl_gens: 50,
        rationale: "read-dep".into(),
    };
    state.substrate.assert_assumption(assumption);
    assert_eq!(state.substrate.assumptions.len(), 1);

    AgentBusEvent::FileWrite {
        agent_id: "writer-c".into(),
        mission_id: None,
        path: path.clone(),
    }
    .apply(&mut state);

    // (i) ClaimViolation emitted for writer-c.
    let violations: Vec<_> = state
        .substrate
        .signals
        .values()
        .filter(|s| s.kind == crate::substrate::SignalKind::ClaimViolation)
        .collect();
    assert!(
        !violations.is_empty(),
        "expected a ClaimViolation signal for the conflicting write",
    );
    assert!(violations.iter().any(|v| v.posted_by == "writer-c"));

    // (ii) Assumption gone.
    assert!(state.substrate.assumptions.is_empty());

    // (iii) Warning signal emitted for invalidation.
    let invalidation = state
        .substrate
        .signals
        .values()
        .find(|s| {
            s.kind == crate::substrate::SignalKind::Warning
                && s.payload.get("reason").and_then(|v| v.as_str())
                    == Some("assumption_invalidated_by_write")
        })
        .expect("expected an invalidation Warning signal");
    assert_eq!(invalidation.posted_by, "writer-c");
    match &invalidation.target {
        crate::substrate::SignalTarget::Agent { agent_id } => {
            assert_eq!(agent_id, "poster-b");
        }
        other => panic!("expected Agent target, got {other:?}"),
    }
}

#[test]
fn file_write_global_assumption_invalidated_by_any_path() {
    let mut state = test_state();
    add_codex_agent(&mut state, "writer-z");

    let assumption = crate::substrate::Assumption {
        id: "assm-global".into(),
        target: crate::substrate::AssumptionTarget::Global,
        fact: serde_json::json!({"depends_on": "everything"}),
        posted_by: "poster-g".into(),
        posted_at_gen: state.substrate.current_generation(),
        ttl_gens: 50,
        rationale: "global-read-dep".into(),
    };
    state.substrate.assert_assumption(assumption);

    AgentBusEvent::FileWrite {
        agent_id: "writer-z".into(),
        mission_id: None,
        path: PathBuf::from("any/random/path.rs"),
    }
    .apply(&mut state);

    assert!(state.substrate.assumptions.is_empty());
    let invalidation = state
        .substrate
        .signals
        .values()
        .find(|s| {
            s.kind == crate::substrate::SignalKind::Warning
                && s.payload.get("reason").and_then(|v| v.as_str())
                    == Some("assumption_invalidated_by_write")
        })
        .expect("expected an invalidation Warning signal for global assumption");
    match &invalidation.target {
        crate::substrate::SignalTarget::Agent { agent_id } => {
            assert_eq!(agent_id, "poster-g");
        }
        other => panic!("expected Agent target, got {other:?}"),
    }
}

#[test]
fn set_mood_event_applies_and_sets_override_lock() {
    use crate::mood::Mood;

    let mut state = test_state();
    state.substrate.generation = 5;
    assert_eq!(state.substrate.mood, Mood::Consolidation);

    AgentBusEvent::SetMood {
        mood: Mood::Defensive,
        source: "user".into(),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.mood, Mood::Defensive);
    assert!(state.substrate.mood_override_until_gen > 0);

    let shift_signal = state
        .substrate
        .signals
        .values()
        .find(|s| {
            s.posted_by == "mood"
                && s.payload.get("reason").and_then(|v| v.as_str()) == Some("mood_manual_override")
        })
        .expect("expected mood_manual_override signal");
    assert_eq!(
        shift_signal.payload.get("source").and_then(|v| v.as_str()),
        Some("user"),
    );
}

#[test]
fn emit_signal_request_mints_id_and_writes_substrate() {
    // The *Request variant delegates id minting to the substrate so external
    // callers (e.g. the nit-mcp back-channel) never see raw counter state.
    // Seed at non-zero gen so the format is observable.
    let mut state = test_state();
    state.substrate.generation = 4;
    let before = state.substrate.signal_counter;

    AgentBusEvent::EmitSignalRequest {
        posted_by: "mcp-agent".into(),
        kind: crate::substrate::SignalKind::Lead,
        target: crate::substrate::SignalTarget::Global,
        payload: serde_json::json!({"hint": "try this"}),
        initial_strength: Some(0.9),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.signals.len(), 1);
    assert_eq!(state.substrate.signal_counter, before + 1);
    let (id, signal) = state.substrate.signals.iter().next().unwrap();
    assert!(id.starts_with("4-mcp-agent-"), "unexpected id: {id}");
    assert_eq!(signal.kind, crate::substrate::SignalKind::Lead);
    assert_eq!(signal.posted_by, "mcp-agent");
    assert_eq!(signal.posted_at_gen, 4);
    assert!((signal.initial_strength - 0.9).abs() < f32::EPSILON * 10.0);
    assert_eq!(signal.payload["hint"], "try this");
}

#[test]
fn assert_claim_request_mints_id_and_honors_mood_ttl() {
    use crate::mood::Mood;

    // Defensive mood has a 1.5x claim_ttl_multiplier — a 4-gen claim should
    // be stretched to 6 on apply. We bypass SetMood to skip the override
    // lock and its synthetic mood_manual_override signal.
    let mut state = test_state();
    state.substrate.mood = Mood::Defensive;
    assert!((state.substrate.mood.modulation().claim_ttl_multiplier - 1.5).abs() < f32::EPSILON);

    let before = state.substrate.claim_counter;
    AgentBusEvent::AssertClaimRequest {
        claimed_by: "mcp-agent".into(),
        kind: crate::substrate::ClaimKind::ExclusiveWrite,
        target: crate::substrate::ClaimTarget::File {
            path: PathBuf::from("/tmp/mcp-test.rs"),
        },
        ttl_gens: 4,
        rationale: "integration-from-mcp".into(),
    }
    .apply(&mut state);

    assert_eq!(state.substrate.claims.len(), 1);
    assert_eq!(state.substrate.claim_counter, before + 1);
    let (id, claim) = state.substrate.claims.iter().next().unwrap();
    assert!(id.starts_with("0-mcp-agent-"), "unexpected id: {id}");
    assert_eq!(claim.ttl_gens, 6, "defensive mood should multiply 4 * 1.5");
    assert_eq!(claim.rationale, "integration-from-mcp");
    assert_eq!(claim.kind, crate::substrate::ClaimKind::ExclusiveWrite);
    // No conflicts → no ClaimViolation signals.
    assert!(state.substrate.signals.is_empty());
}
