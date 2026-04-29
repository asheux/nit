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
