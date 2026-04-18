use super::*;
use crate::agent_bus::AgentBusEvent;
use crate::state::{AgentLane, AgentLaneKind, AgentStatus, AgentTurnState, AppState};
use crate::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};

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
        shadow: false,
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

fn inject_warning(state: &mut AppState, posted_by: &str, posted_at_gen: u64, counter: u64) {
    let id = format!("{posted_at_gen}-{posted_by}-{counter}");
    state.substrate.emit_signal(Signal {
        id,
        kind: SignalKind::Warning,
        posted_by: posted_by.into(),
        posted_at_gen,
        target: SignalTarget::Agent {
            agent_id: posted_by.into(),
        },
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::Value::Null,
    });
}

#[test]
fn framework_invokes_all_registered_observers() {
    let state = test_state();
    let emissions = run_all(&state);
    assert!(emissions.is_empty());
    assert!(REGISTERED_OBSERVERS.len() >= 2);
}

#[test]
fn repeat_failure_emits_help_needed_after_two_warnings() {
    let mut state = test_state();
    inject_warning(&mut state, "a1", 0, 0);
    inject_warning(&mut state, "a1", 0, 1);

    let emissions = (repeat_failure::OBSERVER.run)(&state);
    assert_eq!(emissions.len(), 1);
    let em = &emissions[0];
    assert_eq!(em.kind, SignalKind::HelpNeeded);
    match &em.target {
        SignalTarget::Agent { agent_id } => assert_eq!(agent_id, "a1"),
        other => panic!("expected Agent target, got {other:?}"),
    }
    assert_eq!(em.initial_strength, 1.5);
}

#[test]
fn repeat_failure_silent_with_single_warning() {
    let mut state = test_state();
    inject_warning(&mut state, "a1", 0, 0);

    let emissions = (repeat_failure::OBSERVER.run)(&state);
    assert!(emissions.is_empty());
}

#[test]
fn repeat_failure_self_silencing() {
    let mut state = test_state();
    inject_warning(&mut state, "a1", 0, 0);
    inject_warning(&mut state, "a1", 0, 1);

    // Pre-existing HelpNeeded from the observer should silence re-emission.
    state.substrate.emit_signal(Signal {
        id: "0-observer:repeat_failure-0".into(),
        kind: SignalKind::HelpNeeded,
        posted_by: "observer:repeat_failure".into(),
        posted_at_gen: 0,
        target: SignalTarget::Agent {
            agent_id: "a1".into(),
        },
        initial_strength: OBSERVER_INITIAL_STRENGTH,
        payload: serde_json::Value::Null,
    });

    let emissions = (repeat_failure::OBSERVER.run)(&state);
    assert!(emissions.is_empty());
}

#[test]
fn turn_completed_integration_runs_observers_and_persists() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut dir = std::env::temp_dir();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!(
        "nit-test-observers-integration-{now}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let editor = crate::Buffer::from_str("editor", "", None);
    let notes = crate::Buffer::from_str("notes", "", None);
    let mut state = AppState::new(dir.clone(), editor, notes);
    add_codex_agent(&mut state, "gpt-test");

    // Pre-seed two Warnings posted_by="a1" at gen=0 so the repeat_failure
    // observer fires on the next TurnCompleted tick.
    inject_warning(&mut state, "a1", 0, 0);
    inject_warning(&mut state, "a1", 0, 1);

    AgentBusEvent::TurnCompleted {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "ok".into(),
    }
    .apply(&mut state);

    let reloaded = SubstrateState::load(&dir);
    assert_eq!(reloaded.current_generation(), 1);

    let observer_signals: Vec<_> = reloaded
        .signals
        .values()
        .filter(|s| s.posted_by == "observer:repeat_failure")
        .collect();
    assert!(
        !observer_signals.is_empty(),
        "expected at least one observer:repeat_failure signal, got {:?}",
        reloaded.signals.values().collect::<Vec<_>>()
    );
    let obs = observer_signals[0];
    assert_eq!(obs.kind, SignalKind::HelpNeeded);
    assert_eq!(obs.posted_at_gen, reloaded.current_generation());
}

fn inject_unresolved_dep_warning(
    state: &mut AppState,
    planner_agent: &str,
    missing_dep: &str,
    posted_at_gen: u64,
    counter: u64,
) {
    let posted_by = format!("planner:{planner_agent}");
    let id = format!("{posted_at_gen}-{posted_by}-{counter}");
    state.substrate.emit_signal(Signal {
        id,
        kind: SignalKind::Warning,
        posted_by: posted_by.clone(),
        posted_at_gen,
        target: SignalTarget::Agent {
            agent_id: planner_agent.into(),
        },
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::json!({
            "reason": "unresolved_dep",
            "task_id": "integrate",
            "missing_dep": missing_dep,
        }),
    });
}

#[test]
fn sparse_plan_silent_below_threshold() {
    let mut state = test_state();
    inject_unresolved_dep_warning(&mut state, "alice", "judge", 0, 0);
    inject_unresolved_dep_warning(&mut state, "alice", "judge", 0, 1);
    let emissions = (sparse_plan::OBSERVER.run)(&state);
    assert!(emissions.is_empty());
}

#[test]
fn sparse_plan_emits_help_needed_at_threshold() {
    let mut state = test_state();
    inject_unresolved_dep_warning(&mut state, "alice", "judge", 0, 0);
    inject_unresolved_dep_warning(&mut state, "alice", "judge", 0, 1);
    inject_unresolved_dep_warning(&mut state, "alice", "review", 0, 2);
    let emissions = (sparse_plan::OBSERVER.run)(&state);
    assert_eq!(emissions.len(), 1);
    let em = &emissions[0];
    assert_eq!(em.kind, SignalKind::HelpNeeded);
    match &em.target {
        SignalTarget::Agent { agent_id } => assert_eq!(agent_id, "alice"),
        other => panic!("expected Agent target, got {other:?}"),
    }
    assert_eq!(
        em.payload.get("reason").and_then(|v| v.as_str()),
        Some("sparse_plan")
    );
}

#[test]
fn sparse_plan_self_silences_on_recent_help_needed() {
    let mut state = test_state();
    for i in 0..5 {
        inject_unresolved_dep_warning(&mut state, "alice", "judge", 0, i);
    }
    state.substrate.emit_signal(Signal {
        id: "0-observer:sparse_plan-0".into(),
        kind: SignalKind::HelpNeeded,
        posted_by: "observer:sparse_plan".into(),
        posted_at_gen: 0,
        target: SignalTarget::Agent {
            agent_id: "alice".into(),
        },
        initial_strength: OBSERVER_INITIAL_STRENGTH,
        payload: serde_json::Value::Null,
    });
    let emissions = (sparse_plan::OBSERVER.run)(&state);
    assert!(emissions.is_empty());
}
