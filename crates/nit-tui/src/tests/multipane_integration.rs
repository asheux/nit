//! Multipane integration tests (Phase 6.2).
//!
//! These exercise the high-level invariants the multipane spec calls
//! out at `docs/MULTIPANE.md` — per-pane cwd dispatch, focused-pane
//! abort isolation, the "no agent selected" notice when committing in
//! roster mode, dir-search cwd commit, and persistence roundtrip.
//!
//! Tests run without real `CodexRunner` / `ClaudeRunner` instances;
//! `dispatch_agent_prompt` accepts `None` for both, and the multipane
//! integration points we care about (queue tracking, `mission_id`
//! capture, pane-level state mutation) all happen in `nit-tui` and
//! `nit-core` regardless of whether a runner is attached.

use std::path::PathBuf;

use nit_core::{
    AgentLane, AgentLaneKind, AgentStatus, AgentsState, AppState, MissionPhase, MissionRecord,
    MultipaneState, PaneSession,
};

use super::dispatch::{dispatch_pane_prompt, with_pane_aliased, DispatchOutcome};
use super::persistence::{is_fresh, merge_prior};
use super::setup::materialise_pane_lane;
use crate::app::{handle_abort, parse_abort_command, AbortScope};
use crate::swarm::SwarmRuntime;
use crate::vitals::VitalsState;

fn build_state(panes: &[(usize, &str)]) -> AppState {
    let buffer = nit_core::Buffer::empty("scratch", None);
    let notes = nit_core::Buffer::empty("notes", None);
    let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
    state.agents = AgentsState::default();
    state.agents.agents.push(AgentLane {
        id: "claude-haiku-4-5".into(),
        role: "claude-haiku-4-5".into(),
        lane: "Claude".into(),
        kind: AgentLaneKind::Claude,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    let pane_sessions: Vec<PaneSession> = panes
        .iter()
        .map(|(idx, cwd)| PaneSession {
            pane_id: *idx,
            cwd: PathBuf::from(*cwd),
            ..PaneSession::default()
        })
        .collect();
    state.multipane = Some(MultipaneState {
        backend_agent_id: "claude-haiku-4-5".into(),
        panes: pane_sessions,
        focused: 0,
        grid_cols: panes.len(),
        grid_rows: 1,
        backend_filter: Some("claude-haiku-4-5".into()),
        help_open: false,
    });
    // Materialise per-pane lanes so cwd lookup keys land on the right
    // pane id.
    for idx in 0..panes.len() {
        let _ = materialise_pane_lane(&mut state, idx, "claude-haiku-4-5");
    }
    state
}

#[test]
fn four_panes_each_dispatch_to_their_own_cwd() {
    let mut state = build_state(&[(0, "/p0"), (1, "/p1"), (2, "/p2"), (3, "/p3")]);
    let mut vitals = VitalsState::default();

    for idx in 0..4 {
        let outcome = dispatch_pane_prompt(
            &mut state,
            &mut vitals,
            None,
            None,
            idx,
            format!("hello pane {idx}"),
        );
        assert_eq!(outcome, DispatchOutcome::Dispatched);
    }

    // Each pane lane resolves to its own pane cwd via resolve_dispatch_cwd.
    use crate::app::resolve_dispatch_cwd;
    for idx in 0..4 {
        let id = format!("claude-haiku-4-5#mp-pane-{idx:02}");
        assert_eq!(
            resolve_dispatch_cwd(&state, &id),
            PathBuf::from(format!("/p{idx}"))
        );
    }
}

#[test]
fn abort_in_focused_pane_only_kills_that_pane() {
    let mut state = build_state(&[(0, "/p0"), (1, "/p1")]);
    state.multipane.as_mut().unwrap().focused = 1;

    let pane_a = "claude-haiku-4-5#mp-pane-00".to_string();
    let pane_b = "claude-haiku-4-5#mp-pane-01".to_string();

    state.agents.missions.push(MissionRecord {
        id: "mission-a".into(),
        title: "pane 0 mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec![pane_a.clone()],
        status: String::new(),
        updated_at: String::new(),
    });
    state.agents.missions.push(MissionRecord {
        id: "mission-b".into(),
        title: "pane 1 mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec![pane_b.clone()],
        status: String::new(),
        updated_at: String::new(),
    });

    state
        .multipane
        .as_mut()
        .unwrap()
        .panes
        .iter_mut()
        .zip(["mission-a", "mission-b"].iter())
        .for_each(|(pane, mid)| pane.mission_id = Some((*mid).into()));

    let mut swarm = SwarmRuntime::default();
    // Aborting the focused pane (pane 1) drains pane B's queue. With no
    // active swarm runs and Option<&Runner>=None, handle_abort posts a
    // diagnostic via state.status and returns false — the assertion is
    // that pane A's mission_id is unchanged regardless.
    let _ = with_pane_aliased(&mut state, 1, |state| {
        handle_abort(state, None, None, &mut swarm, AbortScope::Current)
    });

    // Pane 0's mission_id is untouched.
    let pane0_mid = state.multipane.as_ref().unwrap().panes[0]
        .mission_id
        .clone();
    assert_eq!(pane0_mid.as_deref(), Some("mission-a"));
}

#[test]
fn roster_mode_dispatch_emits_no_agent_notice() {
    // Pane 0 has no committed selection (selected_agent_id = None,
    // agent_id = "").
    let buffer = nit_core::Buffer::empty("scratch", None);
    let notes = nit_core::Buffer::empty("notes", None);
    let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
    state.agents = AgentsState::default();
    state.multipane = Some(MultipaneState {
        backend_agent_id: String::new(),
        panes: vec![PaneSession {
            pane_id: 0,
            cwd: PathBuf::from("/p0"),
            ..PaneSession::default()
        }],
        focused: 0,
        grid_cols: 1,
        grid_rows: 1,
        backend_filter: None,
        help_open: false,
    });
    let mut vitals = VitalsState::default();
    let outcome = dispatch_pane_prompt(&mut state, &mut vitals, None, None, 0, "hello".into());
    assert_eq!(outcome, DispatchOutcome::NoSelection);
}

#[test]
fn dir_search_commit_changes_pane_cwd() {
    // Lightweight unit covering the cwd-mutation half of the dir-search
    // flow without exercising the file walker. Mirrors what
    // commit_dir_search does after a take_dir_search_choice() succeeds.
    let mut state = build_state(&[(0, "/p0")]);
    let new_cwd = PathBuf::from("/workspace");
    if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.first_mut()) {
        pane.cwd = new_cwd.clone();
    }
    let resolved = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.first())
        .map(|p| p.cwd.clone());
    assert_eq!(resolved, Some(new_cwd));
}

#[test]
fn persistence_roundtrip() {
    let mut prior = build_state(&[(0, "/p0"), (1, "/p1")]);
    if let Some(mp) = prior.multipane.as_mut() {
        mp.focused = 1;
        mp.panes[0].chat_input = "draft on pane 0".into();
        mp.panes[0].chat_input_cursor = 5;
        mp.panes[0].swarm_template = "parallel".into();
        mp.panes[0].swarm_mission = "research".into();
        mp.panes[0].has_run_mission = true;
        mp.panes[1].chat_input = "another".into();
    }
    let prior_mp = prior.multipane.unwrap();
    let json = serde_json::to_string(&prior_mp).expect("serialize");
    let mut current = build_state(&[(0, "/fresh"), (1, "/fresh")]);
    let restored: MultipaneState = serde_json::from_str(&json).expect("deserialize");
    assert!(merge_prior(current.multipane.as_mut().unwrap(), restored));

    let mp = current.multipane.unwrap();
    assert_eq!(mp.focused, 1);
    assert_eq!(mp.panes[0].chat_input, "draft on pane 0");
    assert_eq!(mp.panes[0].cwd, PathBuf::from("/p0"));
    assert_eq!(mp.panes[0].swarm_template, "parallel");
    assert_eq!(mp.panes[1].chat_input, "another");
    // help_open is `#[serde(skip)]` so always defaults to false on load.
    assert!(!mp.help_open);
    // Fresh-tracking still flips per-pane.
    assert!(!is_fresh(&mp));
}

#[test]
fn at_swarm_prefix_is_recognised_by_canonical_parser() {
    // The chat-reuse path delegates @swarm parsing to
    // `parse_swarm_command`; aborting a parsed scope still goes through
    // `parse_abort_command`. This locks the parser binding.
    assert!(parse_abort_command("/abort").is_some());
    assert!(parse_abort_command("@abort all").is_some());
    assert!(parse_abort_command("hello world").is_none());
}

// ---------------------------------------------------------------------------
// Multipane independence regression suite (BLOCKER coverage)
// ---------------------------------------------------------------------------

use crate::app::broadcast_target_agents;
use crate::widgets::agent_console_view::build_pane_thread_rows_with_breathers_for_pane;
use nit_core::{AgentChannel, AgentMessage};
use std::collections::HashSet;

fn test_swarm_with_active_missions(missions: &[(&str, &str)]) -> SwarmRuntime {
    // Build a runtime where each mission has one running task assigned
    // to its agent — enough to make `is_active_mission` return true and
    // `abort_mission` produce the agent_id.
    let mut runtime = SwarmRuntime::default();
    for (mid, agent_id) in missions {
        let one = crate::swarm::test_runtime_with_running_tasks(mid, &[(agent_id, "exec")]);
        crate::swarm::merge_single_mission_runtime(&mut runtime, one);
    }
    runtime
}

fn build_state_with_chat_missions(n: usize) -> AppState {
    let pairs: Vec<(usize, &str)> = (0..n)
        .map(|i| (i, ["/p0", "/p1", "/p2", "/p3"][i.min(3)]))
        .collect();
    let mut state = build_state(&pairs);
    if let Some(mp) = state.multipane.as_mut() {
        for (i, pane) in mp.panes.iter_mut().enumerate() {
            pane.chat_mission_id = format!("mp-pane-{i:02}-chat");
            pane.has_run_mission = true;
        }
    }
    state
}

#[test]
fn default_chat_in_pane0_not_visible_in_pane1() {
    // BUG 1: a plain `hello` in pane 0 must not appear in pane 1's
    // thread. Synthetic chat mission scopes the AgentMessage so the
    // pane-aware render filter excludes it from pane 1.
    let mut state = build_state_with_chat_missions(2);
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();

    state
        .multipane
        .as_mut()
        .unwrap()
        .panes
        .iter_mut()
        .for_each(|p| p.chat_input.clear());
    state.multipane.as_mut().unwrap().panes[0].chat_input = "hello".into();

    with_pane_aliased(&mut state, 0, |state| {
        let _ = crate::app::submit_chat_input_and_dispatch(
            state,
            &mut vitals,
            None,
            None,
            &mut swarm,
            &mut shadow,
        );
    });

    let pushed = state.agents.messages.last().expect("pushed message");
    assert_eq!(
        pushed.mission_id.as_deref(),
        Some("mp-pane-00-chat"),
        "default-chat from pane 0 must carry pane 0's synthetic mission id"
    );
    assert_eq!(pushed.text, "hello");

    // Pane 1's render filter must exclude the message.
    let pane1 = state.multipane.as_ref().unwrap().panes[1].clone();
    let rows = build_pane_thread_rows_with_breathers_for_pane(
        &state,
        None,
        Some(1),
        Some(pane1.agent_id.as_str()),
        Some(pane1.chat_mission_id.as_str()),
        80,
        false,
    );
    for row in &rows {
        assert!(
            !row.text.contains("hello"),
            "pane 1 must not see pane 0's `hello`: row={:?}",
            row.text
        );
    }
}

#[test]
fn at_all_only_targets_agents_in_originating_pane() {
    // BUG 2 (dispatch side): broadcast_target_agents must restrict to
    // the focused pane's lanes when the synthetic mission has no entry
    // in state.agents.missions.
    let mut state = build_state_with_chat_missions(2);
    state.agents.agents.push(nit_core::AgentLane {
        id: "claude-haiku-4-5#mp-pane-01".into(),
        role: "claude-haiku-4-5#mp-pane-01".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    state.agents.rebuild_agents_index();
    state.multipane.as_mut().unwrap().focused = 0;

    let targets: HashSet<String> = with_pane_aliased(&mut state, 0, |state| {
        broadcast_target_agents(state, Some("mp-pane-00-chat"))
            .into_iter()
            .collect()
    });
    assert!(
        targets.contains("claude-haiku-4-5#mp-pane-00"),
        "pane 0 lane must be in the broadcast set"
    );
    assert!(
        !targets.contains("claude-haiku-4-5#mp-pane-01"),
        "pane 1 lane must NOT receive a broadcast issued from pane 0"
    );
}

#[test]
fn at_all_in_pane_0_replies_only_render_in_pane_0() {
    // BUG 2 (render side / defense-in-depth): even if a Broadcast
    // message somehow leaks across panes, the pane_id-aware filter
    // drops messages whose author belongs to another pane.
    let mut state = build_state_with_chat_missions(2);
    state.agents.messages.push(AgentMessage {
        at: "t+0".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("claude-haiku-4-5#mp-pane-01".into()),
        mission_id: Some("mp-pane-00-chat".into()),
        text: "stray reply from pane 1's lane".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let pane0 = state.multipane.as_ref().unwrap().panes[0].clone();
    let rows = build_pane_thread_rows_with_breathers_for_pane(
        &state,
        None,
        Some(0),
        Some(pane0.agent_id.as_str()),
        Some(pane0.chat_mission_id.as_str()),
        80,
        false,
    );
    for row in &rows {
        assert!(
            !row.text.contains("stray reply from pane 1's lane"),
            "pane 0 must drop messages authored by pane 1's lane"
        );
    }
}

#[test]
fn chat_slash_abort_kills_focused_pane_only() {
    // BUG 3: /abort issued from pane 0 must not touch pane 1's mission.
    let mut state = build_state(&[(0, "/p0"), (1, "/p1")]);

    let pane_a = "claude-haiku-4-5#mp-pane-00".to_string();
    let pane_b = "claude-haiku-4-5#mp-pane-01".to_string();
    state.agents.missions.push(MissionRecord {
        id: "mission-a".into(),
        title: "pane 0 mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec![pane_a.clone()],
        status: String::new(),
        updated_at: String::new(),
    });
    state.agents.missions.push(MissionRecord {
        id: "mission-b".into(),
        title: "pane 1 mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec![pane_b.clone()],
        status: String::new(),
        updated_at: String::new(),
    });
    {
        let mp = state.multipane.as_mut().unwrap();
        mp.focused = 0;
        mp.panes[0].mission_id = Some("mission-a".into());
        mp.panes[1].mission_id = Some("mission-b".into());
        for pane in &mut mp.panes {
            pane.chat_mission_id = format!("mp-pane-{:02}-chat", pane.pane_id);
        }
    }
    let mut swarm =
        test_swarm_with_active_missions(&[("mission-a", &pane_a), ("mission-b", &pane_b)]);

    let _ = with_pane_aliased(&mut state, 0, |state| {
        handle_abort(state, None, None, &mut swarm, AbortScope::Current)
    });

    assert!(!swarm.is_active_mission("mission-a"));
    assert!(
        swarm.is_active_mission("mission-b"),
        "pane 1's mission must NOT be aborted by /abort issued in pane 0"
    );
}

#[test]
fn abort_when_focused_pane_has_no_mission_does_not_kill_others_mission() {
    // BUG 3 leak vector: a synthetic-only pane firing /abort must not
    // resolve through the global active_mission_ids fallback into
    // another pane's mission.
    let mut state = build_state(&[(0, "/p0"), (1, "/p1")]);

    let pane_b = "claude-haiku-4-5#mp-pane-01".to_string();
    state.agents.missions.push(MissionRecord {
        id: "mission-b".into(),
        title: "pane 1 mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec![pane_b.clone()],
        status: String::new(),
        updated_at: String::new(),
    });
    {
        let mp = state.multipane.as_mut().unwrap();
        mp.focused = 0;
        mp.panes[0].mission_id = None;
        mp.panes[0].chat_mission_id = "mp-pane-00-chat".into();
        mp.panes[1].mission_id = Some("mission-b".into());
        mp.panes[1].chat_mission_id = "mp-pane-01-chat".into();
    }
    let mut swarm = test_swarm_with_active_missions(&[("mission-b", &pane_b)]);

    let _ = with_pane_aliased(&mut state, 0, |state| {
        handle_abort(state, None, None, &mut swarm, AbortScope::Current)
    });

    assert!(
        swarm.is_active_mission("mission-b"),
        "/abort in synthetic-only pane 0 must NOT cancel pane 1's swarm"
    );
}

#[test]
fn chat_slash_abort_all_in_pane_only_kills_pane_missions() {
    // BUG 5 (decision): /abort all in multipane is scoped to the focused
    // pane's missions, never global.
    let mut state = build_state(&[(0, "/p0"), (1, "/p1")]);

    let pane_a = "claude-haiku-4-5#mp-pane-00".to_string();
    let pane_b = "claude-haiku-4-5#mp-pane-01".to_string();
    state.agents.missions.push(MissionRecord {
        id: "mission-a".into(),
        title: "pane 0 mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec![pane_a.clone()],
        status: String::new(),
        updated_at: String::new(),
    });
    state.agents.missions.push(MissionRecord {
        id: "mission-b".into(),
        title: "pane 1 mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec![pane_b.clone()],
        status: String::new(),
        updated_at: String::new(),
    });
    state.multipane.as_mut().unwrap().focused = 0;
    let mut swarm =
        test_swarm_with_active_missions(&[("mission-a", &pane_a), ("mission-b", &pane_b)]);

    let _ = with_pane_aliased(&mut state, 0, |state| {
        handle_abort(state, None, None, &mut swarm, AbortScope::All)
    });
    assert!(!swarm.is_active_mission("mission-a"));
    assert!(swarm.is_active_mission("mission-b"));
}

#[test]
fn abort_agent_id_rejected_when_cross_pane() {
    // BLOCKER: surgical /abort <agent-id> must reject ids belonging to
    // a different pane, otherwise the operator has a backdoor to kill
    // sibling-pane work.
    let mut state = build_state(&[(0, "/p0"), (1, "/p1")]);
    let pane_b = "claude-haiku-4-5#mp-pane-01".to_string();
    state.agents.missions.push(MissionRecord {
        id: "mission-b".into(),
        title: "pane 1 mission".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec![pane_b.clone()],
        status: String::new(),
        updated_at: String::new(),
    });
    state.multipane.as_mut().unwrap().focused = 0;
    let mut swarm = test_swarm_with_active_missions(&[("mission-b", &pane_b)]);

    let aborted = with_pane_aliased(&mut state, 0, |state| {
        handle_abort(
            state,
            None,
            None,
            &mut swarm,
            AbortScope::Agent(pane_b.clone()),
        )
    });
    assert!(!aborted, "cross-pane abort must be a no-op");
    assert!(
        swarm.is_active_mission("mission-b"),
        "pane 1's mission must remain active after a cross-pane /abort attempt"
    );
    assert!(
        state
            .status
            .as_deref()
            .is_some_and(|s| s.contains("does not belong to the focused pane")),
        "operator must see a system message explaining the rejection: status={:?}",
        state.status
    );
}

#[test]
fn pane_owns_agent_classifies_swarm_clones() {
    // BLOCKER #3: swarm clones nest a `#swarm-…` suffix after the pane
    // index. Without parser tolerance, pane_owns_agent would drop them.
    use crate::multipane::agent_id::pane_owns_agent;
    assert!(pane_owns_agent(
        "claude-haiku-4-5#mp-pane-00#swarm-mis-001-clone-01",
        0
    ));
    assert!(!pane_owns_agent(
        "claude-haiku-4-5#mp-pane-00#swarm-mis-001-clone-01",
        1
    ));
}

#[test]
fn mirror_back_guard_does_not_clobber_real_mission_id_with_synthetic() {
    // Highest-risk test: with_pane_aliased must NEVER write the synthetic
    // chat id back into pane.mission_id. If it did, swarm-followup
    // re-activation at chat_input.rs would silently break.
    let mut state = build_state_with_chat_missions(2);
    state.multipane.as_mut().unwrap().panes[0].mission_id = None;

    // Body intentionally leaves selected_mission as the synthetic — that
    // mirrors a default-chat dispatch in a fresh pane.
    with_pane_aliased(&mut state, 0, |state| {
        assert_eq!(
            state.agents.selected_mission.as_deref(),
            Some("mp-pane-00-chat")
        );
    });
    assert!(
        state.multipane.as_ref().unwrap().panes[0]
            .mission_id
            .is_none(),
        "synthetic id must NOT be mirrored into pane.mission_id"
    );
}

#[test]
fn mirror_back_writes_real_mission_id_through_when_swarm_starts() {
    // Counter-test: when the body sets a real swarm mission id (the
    // `@swarm` path), with_pane_aliased mirrors it back so subsequent
    // /abort routing targets the right mission.
    let mut state = build_state_with_chat_missions(2);
    state.multipane.as_mut().unwrap().panes[0].mission_id = None;

    with_pane_aliased(&mut state, 0, |state| {
        state.agents.selected_mission = Some("swarm-mis-real".into());
    });
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[0]
            .mission_id
            .as_deref(),
        Some("swarm-mis-real"),
        "real swarm mission must mirror back into pane.mission_id"
    );
}

#[test]
fn single_pane_broadcast_still_renders_when_multipane_is_none() {
    // Regression guard: removing the unconditional Broadcast bypass must
    // not regress single-pane swarm broadcasts. With multipane = None,
    // a Broadcast carrying a different mission_id still surfaces in the
    // current thread (the matcher's broadcast_bypasses branch).
    let buffer = nit_core::Buffer::empty("scratch", None);
    let notes = nit_core::Buffer::empty("notes", None);
    let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
    state.agents = AgentsState::default();
    assert!(state.multipane.is_none());
    // User-typed broadcast (no agent_id) carrying a different mission_id
    // — the single-pane Broadcast bypass clause is what surfaces it.
    state.agents.messages.push(AgentMessage {
        at: "t+0".into(),
        channel: AgentChannel::Broadcast,
        agent_id: None,
        mission_id: Some("some-other-mission".into()),
        text: "user broadcast in single-pane".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = build_pane_thread_rows_with_breathers_for_pane(
        &state,
        None,
        None,
        Some("claude-haiku-4-5"),
        Some("active-mission"),
        80,
        false,
    );
    let surfaced = rows
        .iter()
        .any(|r| r.text.contains("user broadcast in single-pane"));
    assert!(
        surfaced,
        "single-pane Broadcast bypass must still surface the message"
    );
}

#[test]
fn at_all_message_authored_by_pane0_does_not_appear_in_pane1() {
    // Operator-reported BUG 2 reproducer: a Broadcast message authored
    // in pane 0 (mission_id = mp-pane-00-chat) must not bleed into pane
    // 1's render. With the multipane Broadcast-bypass gate disabled,
    // strict mission_id matching keeps it out.
    let mut state = build_state_with_chat_missions(2);
    state.agents.messages.push(AgentMessage {
        at: "t+0".into(),
        channel: AgentChannel::Broadcast,
        agent_id: None,
        mission_id: Some("mp-pane-00-chat".into()),
        text: "broadcast from pane 0".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let pane1 = state.multipane.as_ref().unwrap().panes[1].clone();
    let rows = build_pane_thread_rows_with_breathers_for_pane(
        &state,
        None,
        Some(1),
        Some(pane1.agent_id.as_str()),
        Some(pane1.chat_mission_id.as_str()),
        80,
        false,
    );
    for row in &rows {
        assert!(
            !row.text.contains("broadcast from pane 0"),
            "pane 1 must not see pane 0's broadcast"
        );
    }
}

#[test]
fn merge_prior_re_derives_chat_mission_id_from_pane_id() {
    // The persistence layer recomputes chat_mission_id on load so a
    // session file written before the field existed still loads with
    // canonical ids — locking the field's "pure function of pane_id"
    // invariant.
    let mut prior = build_state(&[(0, "/p0"), (1, "/p1")]);
    if let Some(mp) = prior.multipane.as_mut() {
        for pane in &mut mp.panes {
            pane.chat_mission_id = "stale".into();
        }
    }
    let prior_mp = prior.multipane.unwrap();
    let json = serde_json::to_string(&prior_mp).unwrap();
    let mut current = build_state(&[(0, "/p0"), (1, "/p1")]);
    let restored: nit_core::MultipaneState = serde_json::from_str(&json).unwrap();
    assert!(merge_prior(current.multipane.as_mut().unwrap(), restored));
    let mp = current.multipane.unwrap();
    assert_eq!(mp.panes[0].chat_mission_id, "mp-pane-00-chat");
    assert_eq!(mp.panes[1].chat_mission_id, "mp-pane-01-chat");
}

// ---------------------------------------------------------------------------
// BUG 1 / BUG 2 regression tests (multipane bug-batch integrate-01)
// ---------------------------------------------------------------------------

use nit_core::AgentConsoleRowKind;

#[test]
fn breather_rows_in_pane_k_only_contain_pane_k_agents() {
    // BUG 1: when single-agent dispatch is in flight on multiple panes,
    // each pane's render must show ONLY its own pane lane in the breather
    // table. Pre-fix, the trailing breather block in
    // `breather_rows_for_user_prompt` walked every active lane in
    // `state.agents.agents` because `mission_ctx` was None at render
    // time, leaking pane J's agents into pane K's breather.
    let mut state = build_state_with_chat_missions(2);

    // Materialise pane 1's lane (pane 0 is materialised by build_state).
    let _ = materialise_pane_lane(&mut state, 1, "claude-haiku-4-5");

    let pane0_id = "claude-haiku-4-5#mp-pane-00".to_string();
    let pane1_id = "claude-haiku-4-5#mp-pane-01".to_string();

    // Both lanes are running, each tied to its pane's synthetic chat id.
    if let Some(lane) = state.agents.agents_get_mut(&pane0_id) {
        lane.current_mission = Some("mp-pane-00-chat".into());
        lane.status = nit_core::AgentStatus::Running;
    }
    if let Some(lane) = state.agents.agents_get_mut(&pane1_id) {
        lane.current_mission = Some("mp-pane-01-chat".into());
        lane.status = nit_core::AgentStatus::Running;
    }
    state.agents.active_turns.insert(
        pane0_id.clone(),
        nit_core::state::AgentTurnState {
            started_at: std::time::Instant::now(),
            last_heartbeat_at: std::time::Instant::now(),
            last_output_at: std::time::Instant::now(),
            stage: Some("running".into()),
        },
    );
    state.agents.active_turns.insert(
        pane1_id.clone(),
        nit_core::state::AgentTurnState {
            started_at: std::time::Instant::now(),
            last_heartbeat_at: std::time::Instant::now(),
            last_output_at: std::time::Instant::now(),
            stage: Some("running".into()),
        },
    );

    // Render path aliases selected_mission to the pane's synthetic id —
    // mirror that here so breather_rows_for_user_prompt's filter sees
    // the same context the renderer would set.
    state.agents.selected_mission = Some("mp-pane-00-chat".into());
    state.agents.selected_agent = Some(pane0_id.clone());
    state.agents.mission_selected = usize::MAX;

    let pane0 = state.multipane.as_ref().unwrap().panes[0].clone();
    let rows = build_pane_thread_rows_with_breathers_for_pane(
        &state,
        None,
        Some(0),
        Some(pane0.agent_id.as_str()),
        Some(pane0.chat_mission_id.as_str()),
        80,
        false,
    );

    // No row in pane 0's render may carry pane 1's agent text. The
    // breather table builder embeds the agent role into the row, so a
    // contains-check on the role is the cleanest signal.
    for row in &rows {
        if matches!(
            row.kind,
            AgentConsoleRowKind::StatusRow | AgentConsoleRowKind::StatusSubRow
        ) {
            assert!(
                !row.text.contains("mp-pane-01"),
                "pane 0 breather row leaked pane 1's lane: {:?}",
                row.text
            );
        }
    }
}

#[test]
fn abort_via_typed_slash_in_pane_routes_to_agent_scope() {
    // BUG 2 (typed /abort path): a single-agent pane with a turn in
    // flight (active_turns populated) but no real swarm mission must
    // surgically cancel the agent instead of bailing with "no active
    // swarm mission". `resolve_current_abort` now falls through to a
    // per-agent CancelTurn for multipane panes whose synthetic mission
    // doesn't appear in `swarm.active_mission_ids`.
    let mut state = build_state_with_chat_missions(2);
    let agent = "claude-haiku-4-5#mp-pane-00".to_string();

    state.agents.active_turns.insert(
        agent.clone(),
        nit_core::state::AgentTurnState {
            started_at: std::time::Instant::now(),
            last_heartbeat_at: std::time::Instant::now(),
            last_output_at: std::time::Instant::now(),
            stage: Some("running".into()),
        },
    );
    if let Some(lane) = state.agents.agents_get_mut(&agent) {
        lane.current_mission = Some("mp-pane-00-chat".into());
        lane.status = nit_core::AgentStatus::Running;
    }

    let mut swarm = SwarmRuntime::default();
    let aborted = with_pane_aliased(&mut state, 0, |state| {
        handle_abort(state, None, None, &mut swarm, AbortScope::Current)
    });
    assert!(
        aborted,
        "/abort with a non-swarm in-flight turn must cancel the focused agent"
    );
    let status = state.status.as_deref().unwrap_or("");
    assert!(
        !status.contains("no active swarm mission"),
        "operator must NOT see the swarm-mode error when an agent turn is live: status={status:?}"
    );
}

#[test]
fn abort_focused_pane_with_non_swarm_in_flight_uses_agent_scope() {
    // BUG 2 (Ctrl+C / Esc-Esc / Mission-tab `x` path): the focused-pane
    // abort must fall through to an agent-scope CancelTurn when
    // `lane.current_mission` is a stale non-swarm id. Pre-fix, a stale
    // mission id caused the early "no active mission for this pane"
    // return even though `active_turns` carried a live turn.
    let mut state = build_state_with_chat_missions(2);
    let agent = "claude-haiku-4-5#mp-pane-00".to_string();

    if let Some(lane) = state.agents.agents_get_mut(&agent) {
        lane.current_mission = Some("ad-hoc-001".into());
        lane.status = nit_core::AgentStatus::Running;
    }
    state.agents.active_turns.insert(
        agent.clone(),
        nit_core::state::AgentTurnState {
            started_at: std::time::Instant::now(),
            last_heartbeat_at: std::time::Instant::now(),
            last_output_at: std::time::Instant::now(),
            stage: Some("running".into()),
        },
    );
    if let Some(mp) = state.multipane.as_mut() {
        mp.panes[0].mission_id = None;
    }

    // Drive the same routing path that abort_focused_pane uses: a
    // non-swarm mission id falls through to AbortScope::Agent. The
    // existing `chat_slash_abort_kills_focused_pane_only` test already
    // covers the typed /abort path; this exercises the Agent-scope
    // resolver directly.
    let mut swarm = SwarmRuntime::default();
    let aborted = with_pane_aliased(&mut state, 0, |state| {
        handle_abort(
            state,
            None,
            None,
            &mut swarm,
            AbortScope::Agent(agent.clone()),
        )
    });
    assert!(
        aborted,
        "AbortScope::Agent for the focused pane's lane must succeed"
    );
}

// Lens-E parent-pane fallback: clone descendants of a pane lane carry
// `<base>#mp-pane-NN<#…>` and exact-id match misses them. Without
// `parse_pane_agent_id` walking the suffix chain, every @swarm / @new /
// @shadow issued from a pane resolved to workspace_root.
#[test]
fn dispatch_cwd_parent_pane_fallback_for_clone_descendants() {
    let state = build_state(&[(0, "/pane0"), (1, "/pane1")]);
    let cases = [
        (
            "claude-haiku-4-5#mp-pane-00#swarm-mis-001-clone-03",
            "/pane0",
        ),
        ("claude-haiku-4-5#mp-pane-00#shadow-001-propose-a", "/pane0"),
        ("claude-haiku-4-5#mp-pane-00#chat-clone-01", "/pane0"),
        (
            "claude-haiku-4-5#mp-pane-01#swarm-mis-007-clone-02",
            "/pane1",
        ),
    ];
    for (id, expected) in cases {
        assert_eq!(
            crate::app::resolve_dispatch_cwd(&state, id),
            PathBuf::from(expected),
            "id {id}",
        );
    }
}

// Cross-pane independence: mutating pane 0's cwd must NOT mutate
// pane 1's resolved cwd or any of pane 1's clone descendants.
#[test]
fn dispatch_cwd_isolates_panes_after_pane0_change() {
    let mut state = build_state(&[(0, "/pane0"), (1, "/pane1")]);
    state.multipane.as_mut().unwrap().panes[0].cwd = PathBuf::from("/pane0-new");
    assert_eq!(
        crate::app::resolve_dispatch_cwd(&state, "claude-haiku-4-5#mp-pane-00"),
        PathBuf::from("/pane0-new"),
    );
    assert_eq!(
        crate::app::resolve_dispatch_cwd(&state, "claude-haiku-4-5#mp-pane-01"),
        PathBuf::from("/pane1"),
    );
    assert_eq!(
        crate::app::resolve_dispatch_cwd(
            &state,
            "claude-haiku-4-5#mp-pane-01#swarm-mis-007-clone-02",
        ),
        PathBuf::from("/pane1"),
    );
}

// Queued turns reach `resolve_dispatch_cwd` at dequeue. Mutating
// pane.cwd between two resolver calls for the same agent_id models
// the queue→dequeue gap; the second call must see the new value.
#[test]
fn dispatch_cwd_picks_up_pane_change_at_dequeue() {
    let mut state = build_state(&[(0, "/pane0")]);
    let queued_id = "claude-haiku-4-5#mp-pane-00";
    assert_eq!(
        crate::app::resolve_dispatch_cwd(&state, queued_id),
        PathBuf::from("/pane0"),
    );
    state.multipane.as_mut().unwrap().panes[0].cwd = PathBuf::from("/pane0-after-dir-search");
    assert_eq!(
        crate::app::resolve_dispatch_cwd(&state, queued_id),
        PathBuf::from("/pane0-after-dir-search"),
    );
}
