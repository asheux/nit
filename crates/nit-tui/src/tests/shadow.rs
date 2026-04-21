//! Tests for the single-agent shadow pipeline.
//!
//! Parser/helper checks are lightweight unit tests. The DAG tests drive the
//! full pipeline — start → propose-a + propose-b → judge → review → main —
//! using an `AppState` plus a minimal mock roster so they don't depend on
//! any runner.

use nit_core::state::AgentTurnState;
use nit_core::{AgentBusEvent, AgentLane, AgentLaneKind, AgentStatus, AppState, Buffer};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::shadow::{
    parse_shadow_command, parse_shadow_lane_id, shadow_lane_id, shadow_stage_label_from_state,
    should_auto_enable_shadows, ShadowRuntime, SHADOW_ROLES,
};

#[test]
fn parse_shadow_command_accepts_explicit_prefix() {
    let cmd = parse_shadow_command("@shadow refactor core").unwrap();
    assert_eq!(cmd.prompt, "refactor core");
}

#[test]
fn parse_shadow_command_rejects_embedded_prefix() {
    assert!(parse_shadow_command("please @shadow foo").is_none());
    assert!(parse_shadow_command("@shadows foo").is_none());
    assert!(parse_shadow_command("@shadow").is_none());
}

#[test]
fn parse_shadow_command_tolerates_leading_whitespace() {
    let cmd = parse_shadow_command("  @shadow do it").unwrap();
    assert_eq!(cmd.prompt, "do it");
}

#[test]
fn should_auto_enable_shadows_triggers_on_keyword() {
    assert!(should_auto_enable_shadows("Refactor the widget module"));
    assert!(should_auto_enable_shadows("rewrite this function please"));
    assert!(should_auto_enable_shadows("Implement SSE streaming"));
}

#[test]
fn should_auto_enable_shadows_triggers_on_length() {
    let long = "a".repeat(501);
    assert!(should_auto_enable_shadows(&long));
}

#[test]
fn should_auto_enable_shadows_is_quiet_for_short_questions() {
    assert!(!should_auto_enable_shadows("what does this do?"));
    assert!(!should_auto_enable_shadows("fix typo"));
    assert!(!should_auto_enable_shadows("why is the test flaky?"));
}

#[test]
fn shadow_lane_id_roundtrip() {
    let id = shadow_lane_id("codex", "01", "propose-a");
    assert_eq!(id, "codex#shadow-01-propose-a");
    let (base, run_id, role) = parse_shadow_lane_id(&id).unwrap();
    assert_eq!(base, "codex");
    assert_eq!(run_id, "01");
    assert_eq!(role, "propose-a");
}

#[test]
fn parse_shadow_lane_id_handles_roles_with_dashes() {
    let id = "claude-main#shadow-07-propose-b";
    let (base, run_id, role) = parse_shadow_lane_id(id).unwrap();
    assert_eq!(base, "claude-main");
    assert_eq!(run_id, "07");
    assert_eq!(role, "propose-b");
}

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
            HashSet::new(),
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
    rt.start(
        &mut state,
        "codex-main".into(),
        "prompt".into(),
        None,
        None,
        HashSet::new(),
    )
    .expect("first start");
    assert!(rt
        .start(
            &mut state,
            "codex-main".into(),
            "prompt 2".into(),
            None,
            None,
            HashSet::new(),
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
        .start(
            &mut state,
            "codex-main".into(),
            "p".into(),
            None,
            None,
            HashSet::new(),
        )
        .is_none());

    assert!(rt
        .start(
            &mut state,
            "does-not-exist".into(),
            "p".into(),
            None,
            None,
            HashSet::new(),
        )
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
        HashSet::new(),
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
        HashSet::new(),
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
        HashSet::new(),
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

// Reviewer finishing while the main agent is still mid-turn must defer the
// shadow-augmented dispatch. Otherwise the dispatch gets queued and the next
// unrelated TurnCompleted on main fires premature cleanup (and misattributes
// responses via turn_prompt_idx).
#[test]
fn reviewer_completion_defers_when_main_is_busy() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();
    rt.start(
        &mut state,
        "codex-main".into(),
        "implement feature".into(),
        None,
        Some(5),
        HashSet::new(),
    )
    .unwrap();

    let a_id = shadow_lane_id("codex-main", "01", "propose-a");
    let b_id = shadow_lane_id("codex-main", "01", "propose-b");
    let j_id = shadow_lane_id("codex-main", "01", "judge");
    let r_id = shadow_lane_id("codex-main", "01", "review");

    let _ = rt.handle_event_outcome(&mut state, &completed_event(&a_id, "plan A"));
    let _ = rt.handle_event_outcome(&mut state, &completed_event(&b_id, "plan B"));
    let _ = rt.handle_event_outcome(&mut state, &completed_event(&j_id, "judged"));

    // Main has an in-flight turn unrelated to shadows.
    state
        .agents
        .active_turns
        .insert("codex-main".into(), active_turn_state());

    // Reviewer finishing does NOT dispatch the main agent yet — it parks.
    let out = rt.handle_event_outcome(&mut state, &completed_event(&r_id, "reviewed"));
    assert!(
        out.dispatches.is_empty(),
        "reviewer completion must defer when main is busy"
    );
    assert!(rt.has_run_for("codex-main"), "run must still be alive");
    // Shadow lanes are still present.
    for role in SHADOW_ROLES {
        let id = shadow_lane_id("codex-main", "01", role);
        assert!(state.agents.agents.iter().any(|l| l.id == id));
    }

    // Prior turn clears → main idle → deferred dispatch fires now.
    state.agents.active_turns.remove("codex-main");
    let out = rt.handle_event_outcome(&mut state, &completed_event("codex-main", "Y1 reply"));
    assert_eq!(out.dispatches.len(), 1);
    let d = &out.dispatches[0];
    assert_eq!(d.agent_id, "codex-main");
    assert!(d.prompt.contains("SHADOW CONTEXT"));
    assert!(d.prompt.contains("plan A"));
    assert!(d.prompt.contains("plan B"));
    assert!(d.prompt.contains("judged"));
    assert!(d.prompt.contains("reviewed"));
    assert!(d.prompt.contains("implement feature"));
    assert_eq!(d.prompt_msg_idx, Some(5));

    // Shadow-augmented turn completes → cleanup, NOT on the prior Y1 completion.
    let out = rt.handle_event_outcome(&mut state, &completed_event("codex-main", "final"));
    assert!(out.dispatches.is_empty());
    assert!(!rt.has_run_for("codex-main"));
    for role in SHADOW_ROLES {
        let id = shadow_lane_id("codex-main", "01", role);
        assert!(
            state.agents.agents.iter().all(|l| l.id != id),
            "shadow lane '{role}' not cleaned up"
        );
    }
}

// Passing prescan_paths to start parks the proposer dispatches until the
// genome reports land. Caller gets empty dispatches back; after all pending
// paths clear via note_prescan_result, the proposers are released.
#[test]
fn start_with_prescan_paths_parks_proposers() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();

    let scope = PathBuf::from("/tmp/fake-scope.rs");
    let mut prescan = HashSet::new();
    prescan.insert(scope.clone());

    let dispatches = rt
        .start(
            &mut state,
            "codex-main".into(),
            "refactor it".into(),
            None,
            Some(0),
            prescan,
        )
        .expect("start succeeds");
    assert!(
        dispatches.is_empty(),
        "proposers must be parked when prescan is pending"
    );
    assert!(rt.has_run_for("codex-main"));

    // Runtime hands out the path once for worker dispatch, not repeatedly.
    let first = rt.take_pending_prescan_paths();
    assert_eq!(first, vec![scope.clone()]);
    let second = rt.take_pending_prescan_paths();
    assert!(
        second.is_empty(),
        "already-dispatched paths must not re-emit"
    );

    // Result arrives → proposers released.
    let released = rt.note_prescan_result(&scope);
    assert_eq!(released.len(), 2, "both proposers must be released");
    assert!(released.iter().any(|d| d.agent_id.contains("propose-a")));
    assert!(released.iter().any(|d| d.agent_id.contains("propose-b")));

    // Subsequent completion events should drive the rest of the pipeline
    // as normal (proposers → judge → review → main).
    let ev = completed_event(&shadow_lane_id("codex-main", "01", "propose-a"), "plan A");
    let _ = rt.handle_event_outcome(&mut state, &ev);
    let ev = completed_event(&shadow_lane_id("codex-main", "01", "propose-b"), "plan B");
    let out = rt.handle_event_outcome(&mut state, &ev);
    assert_eq!(out.dispatches.len(), 1, "judge should dispatch");
    assert!(out.dispatches[0].agent_id.contains("judge"));
}

#[test]
fn empty_prescan_paths_returns_proposers_immediately() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();
    let dispatches = rt
        .start(
            &mut state,
            "codex-main".into(),
            "do it".into(),
            None,
            None,
            HashSet::new(),
        )
        .expect("start succeeds");
    assert_eq!(dispatches.len(), 2);
    assert!(rt.take_pending_prescan_paths().is_empty());
}

// Main completing an unrelated turn during Proposing/Judging/Reviewing stages
// must NOT trigger cleanup and must NOT be mistaken for the shadow turn.
#[test]
fn main_completion_during_proposing_does_not_clean_up() {
    let mut state = make_state_with_main_agent("codex-main");
    let mut rt = ShadowRuntime::new();
    rt.start(
        &mut state,
        "codex-main".into(),
        "implement feature".into(),
        None,
        Some(3),
        HashSet::new(),
    )
    .unwrap();

    let out = rt.handle_event_outcome(&mut state, &completed_event("codex-main", "unrelated Y1"));
    assert!(out.dispatches.is_empty());
    assert!(rt.has_run_for("codex-main"), "run must survive");
    for role in SHADOW_ROLES {
        let id = shadow_lane_id("codex-main", "01", role);
        assert!(state.agents.agents.iter().any(|l| l.id == id));
    }
}
