//! Integration tests for the single-agent shadow pipeline.
//!
//! These tests drive the full DAG: start → propose-a + propose-b → judge →
//! review → main agent, cleaning up along the way. They use `AppState` plus a
//! minimal mock roster so they don't depend on any runner.

use nit_core::state::AgentTurnState;
use nit_core::{AgentBusEvent, AgentLane, AgentLaneKind, AgentStatus, AppState, Buffer};
use std::path::PathBuf;
use std::time::Instant;

use crate::shadow::{
    parse_shadow_lane_id, shadow_lane_id, shadow_stage_label_from_state, ShadowRuntime,
    SHADOW_ROLES,
};

fn make_state_with_main_agent(id: &str) -> AppState {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        id: id.into(),
        role: "coder".into(),
        lane: "Codex".into(),
        kind: AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    state.agents.selected_agent = Some(id.into());
    state
}

fn completed_event(agent_id: &str, message: &str) -> AgentBusEvent {
    AgentBusEvent::TurnCompleted {
        agent_id: agent_id.into(),
        mission_id: None,
        message: message.into(),
        thread_id: None,
        token_count: None,
    }
}

fn active_turn_state() -> AgentTurnState {
    let now = Instant::now();
    AgentTurnState {
        started_at: now,
        last_heartbeat_at: now,
        last_output_at: now,
        stage: None,
    }
}

#[test]
fn start_creates_four_shadow_clones_and_returns_two_proposers() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();

    let dispatches = rt
        .start(
            &mut state,
            "codex-main".into(),
            "refactor everything".into(),
            None,
            Some(0),
        )
        .expect("start succeeds");

    // Two proposer dispatches.
    assert_eq!(dispatches.len(), 2);
    assert!(dispatches.iter().any(|d| d.agent_id.contains("propose-a")));
    assert!(dispatches.iter().any(|d| d.agent_id.contains("propose-b")));

    // All four shadow lanes exist and are marked hidden.
    for role in SHADOW_ROLES {
        let id = shadow_lane_id("codex-main", "01", role);
        let lane = state
            .agents
            .agents
            .iter()
            .find(|l| l.id == id)
            .unwrap_or_else(|| panic!("shadow lane for role '{role}' missing"));
        assert!(lane.shadow, "role '{role}' not marked shadow");
    }

    assert!(rt.has_run_for("codex-main"));
    assert!(rt.is_shadow_agent(&shadow_lane_id("codex-main", "01", "judge")));
    assert!(!rt.is_shadow_agent("codex-main"));
}

#[test]
fn start_rejects_duplicate_run_for_same_agent() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();
    rt.start(&mut state, "codex-main".into(), "prompt".into(), None, None)
        .expect("first start");
    assert!(rt
        .start(
            &mut state,
            "codex-main".into(),
            "prompt 2".into(),
            None,
            None,
        )
        .is_none());
}

#[test]
fn start_rejects_unknown_or_non_dispatchable_main_agent() {
    let mut state = make_state_with_main_agent("codex-main");
    // mark as non-dispatchable
    state.agents.agents[0].kind = AgentLaneKind::Mock;
    let mut rt = ShadowRuntime::new();
    assert!(rt
        .start(&mut state, "codex-main".into(), "p".into(), None, None,)
        .is_none());

    assert!(rt
        .start(&mut state, "does-not-exist".into(), "p".into(), None, None,)
        .is_none());
}

#[test]
fn full_dag_proposers_then_judge_then_review_then_main() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();
    rt.start(
        &mut state,
        "codex-main".into(),
        "implement feature".into(),
        None,
        Some(42),
    )
    .unwrap();

    let a_id = shadow_lane_id("codex-main", "01", "propose-a");
    let b_id = shadow_lane_id("codex-main", "01", "propose-b");
    let j_id = shadow_lane_id("codex-main", "01", "judge");
    let r_id = shadow_lane_id("codex-main", "01", "review");

    // First proposer finishes — no dispatch yet (waiting on both).
    let ev = completed_event(&a_id, "plan A");
    let out = rt.handle_event_outcome(&mut state, &ev);
    assert!(out.dispatches.is_empty());

    // Second proposer finishes — dispatches judge.
    let ev = completed_event(&b_id, "plan B");
    let out = rt.handle_event_outcome(&mut state, &ev);
    assert_eq!(out.dispatches.len(), 1);
    assert_eq!(out.dispatches[0].agent_id, j_id);
    assert!(out.dispatches[0].prompt.contains("plan A"));
    assert!(out.dispatches[0].prompt.contains("plan B"));

    // Judge finishes — dispatches reviewer.
    let ev = completed_event(&j_id, "judged plan");
    let out = rt.handle_event_outcome(&mut state, &ev);
    assert_eq!(out.dispatches.len(), 1);
    assert_eq!(out.dispatches[0].agent_id, r_id);
    assert!(out.dispatches[0].prompt.contains("judged plan"));

    // Reviewer finishes — dispatches main agent with full augmented prompt.
    let ev = completed_event(&r_id, "reviewed plan");
    let out = rt.handle_event_outcome(&mut state, &ev);
    assert_eq!(out.dispatches.len(), 1);
    let final_dispatch = &out.dispatches[0];
    assert_eq!(final_dispatch.agent_id, "codex-main");
    assert!(final_dispatch.prompt.contains("SHADOW CONTEXT"));
    assert!(final_dispatch.prompt.contains("plan A"));
    assert!(final_dispatch.prompt.contains("plan B"));
    assert!(final_dispatch.prompt.contains("judged plan"));
    assert!(final_dispatch.prompt.contains("reviewed plan"));
    assert!(final_dispatch.prompt.contains("implement feature"));
    assert_eq!(final_dispatch.prompt_msg_idx, Some(42));

    // Shadow lanes still present during finalization.
    for role in SHADOW_ROLES {
        let id = shadow_lane_id("codex-main", "01", role);
        assert!(state.agents.agents.iter().any(|l| l.id == id));
    }

    // Main agent finishes — shadow lanes are torn down and the run removed.
    let ev = completed_event("codex-main", "final answer");
    let out = rt.handle_event_outcome(&mut state, &ev);
    assert!(out.dispatches.is_empty());
    assert!(!rt.has_run_for("codex-main"));
    for role in SHADOW_ROLES {
        let id = shadow_lane_id("codex-main", "01", role);
        assert!(
            state.agents.agents.iter().all(|l| l.id != id),
            "shadow lane '{role}' leaked"
        );
    }
}

#[test]
fn shadow_failure_falls_back_to_unaugmented_main_dispatch() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();
    rt.start(
        &mut state,
        "codex-main".into(),
        "do the thing".into(),
        None,
        Some(7),
    )
    .unwrap();

    let a_id = shadow_lane_id("codex-main", "01", "propose-a");
    let fail = AgentBusEvent::TurnFailed {
        agent_id: a_id,
        mission_id: None,
        message: "boom".into(),
        thread_id: None,
        token_count: None,
    };
    let out = rt.handle_event_outcome(&mut state, &fail);
    // Falls back: dispatch main agent directly with original prompt.
    assert_eq!(out.dispatches.len(), 1);
    assert_eq!(out.dispatches[0].agent_id, "codex-main");
    assert_eq!(out.dispatches[0].prompt, "do the thing");
    assert_eq!(out.dispatches[0].prompt_msg_idx, Some(7));
    // All shadow lanes removed after abort.
    for role in SHADOW_ROLES {
        let id = shadow_lane_id("codex-main", "01", role);
        assert!(state.agents.agents.iter().all(|l| l.id != id));
    }
    assert!(!rt.has_run_for("codex-main"));
}

#[test]
fn stage_label_reflects_current_active_shadow() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();
    rt.start(
        &mut state,
        "codex-main".into(),
        "implement feature".into(),
        None,
        None,
    )
    .unwrap();

    // No active turns yet → Finalizing (lanes exist, none active).
    assert_eq!(
        shadow_stage_label_from_state(&state, None),
        Some("Finalizing")
    );
    assert_eq!(
        shadow_stage_label_from_state(&state, Some("codex-main")),
        Some("Finalizing")
    );
    // A different main agent id has no shadow lanes → None.
    assert_eq!(shadow_stage_label_from_state(&state, Some("other")), None);

    // Simulate proposer-a running.
    let a_id = shadow_lane_id("codex-main", "01", "propose-a");
    state
        .agents
        .active_turns
        .insert(a_id.clone(), active_turn_state());
    assert_eq!(
        shadow_stage_label_from_state(&state, None),
        Some("Proposing")
    );

    // Proposer done, judge running.
    state.agents.active_turns.remove(&a_id);
    let j_id = shadow_lane_id("codex-main", "01", "judge");
    state
        .agents
        .active_turns
        .insert(j_id.clone(), active_turn_state());
    assert_eq!(shadow_stage_label_from_state(&state, None), Some("Judging"));

    // Judge done, reviewer running.
    state.agents.active_turns.remove(&j_id);
    let r_id = shadow_lane_id("codex-main", "01", "review");
    state.agents.active_turns.insert(r_id, active_turn_state());
    assert_eq!(
        shadow_stage_label_from_state(&state, None),
        Some("Reviewing")
    );
}

#[test]
fn shadow_lane_id_parses_back_correctly_for_all_roles() {
    for role in SHADOW_ROLES {
        let id = shadow_lane_id("base", "42", role);
        let (base, run, parsed_role) = parse_shadow_lane_id(&id).expect("parse");
        assert_eq!(base, "base");
        assert_eq!(run, "42");
        assert_eq!(&parsed_role, role);
    }
}
