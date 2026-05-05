use super::*;
use nit_core::{AgentLane, AgentLaneKind, AgentStatus, AgentsState};

fn fixture_state(backend_id: &str) -> AppState {
    let buffer = nit_core::Buffer::empty("scratch", None);
    let notes = nit_core::Buffer::empty("notes", None);
    let mut state = AppState::new(PathBuf::from("/tmp"), buffer, notes);
    state.agents = AgentsState::default();
    state.agents.agents.push(AgentLane {
        id: backend_id.into(),
        role: backend_id.into(),
        lane: "Claude".into(),
        kind: AgentLaneKind::Claude,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    state
}

fn fixture_state_with_two_backends() -> AppState {
    let mut state = fixture_state("claude-haiku-4-5");
    state.agents.agents.push(AgentLane {
        id: "gpt-5".into(),
        role: "gpt-5".into(),
        lane: "Codex".into(),
        kind: AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    state
}

#[test]
fn install_pre_picks_when_specific_backend() {
    let mut state = fixture_state("claude-haiku-4-5");
    install_filtered(
        &mut state,
        Some("claude-haiku-4-5"),
        4,
        PathBuf::from("/work"),
    )
    .expect("ok");

    let mp = state.multipane.as_ref().expect("set");
    assert_eq!(mp.panes.len(), 4);
    assert_eq!(mp.backend_filter.as_deref(), Some("claude-haiku-4-5"));
    for (i, pane) in mp.panes.iter().enumerate() {
        assert_eq!(pane.pane_id, i);
        let want = format!("claude-haiku-4-5#mp-pane-{i:02}");
        assert_eq!(pane.agent_id, want);
        assert_eq!(pane.selected_agent_id.as_deref(), Some(want.as_str()));
        assert_eq!(pane.cwd, PathBuf::from("/work"));
    }
    assert_eq!(state.agents.agents.len(), 5);
}

#[test]
fn install_returns_err_when_specific_backend_missing() {
    let mut state = fixture_state("claude-haiku-4-5");
    let result = install_filtered(&mut state, Some("bogus"), 4, PathBuf::from("/work"));
    assert!(result.is_err());
    assert!(state.multipane.is_none());
}

#[test]
fn install_filtered_with_no_backend_leaves_roster_intact() {
    let mut state = fixture_state_with_two_backends();
    let canonical_len = state.agents.agents.len();
    install_filtered(&mut state, None, 4, PathBuf::from("/work")).expect("ok");

    let mp = state.multipane.as_ref().expect("set");
    assert_eq!(mp.panes.len(), 4);
    assert!(mp.backend_filter.is_none());
    assert_eq!(state.agents.agents.len(), canonical_len);
    for pane in &mp.panes {
        assert!(pane.selected_agent_id.is_none());
        assert!(pane.agent_id.is_empty());
    }
}

#[test]
fn install_filtered_with_family_backend_records_filter_only() {
    let mut state = fixture_state_with_two_backends();
    let canonical_len = state.agents.agents.len();
    install_filtered(&mut state, Some("claude"), 4, PathBuf::from("/work")).expect("ok");

    let mp = state.multipane.as_ref().expect("set");
    assert_eq!(mp.backend_filter.as_deref(), Some("claude"));
    assert_eq!(state.agents.agents.len(), canonical_len);
    for pane in &mp.panes {
        assert!(pane.selected_agent_id.is_none());
        assert!(pane.agent_id.is_empty());
    }
}

#[test]
fn materialise_pane_lane_creates_unique_lane_per_pane() {
    let mut state = fixture_state_with_two_backends();
    install_filtered(&mut state, None, 2, PathBuf::from("/work")).expect("ok");

    let id_a = materialise_pane_lane(&mut state, 0, "claude-haiku-4-5").expect("id 0");
    let id_b = materialise_pane_lane(&mut state, 1, "claude-haiku-4-5").expect("id 1");
    assert_ne!(id_a, id_b);
    assert!(state.agents.agents.iter().any(|l| l.id == id_a));
    assert!(state.agents.agents.iter().any(|l| l.id == id_b));

    let mp = state.multipane.as_ref().expect("set");
    assert_eq!(
        mp.panes[0].selected_agent_id.as_deref(),
        Some(id_a.as_str())
    );
    assert_eq!(
        mp.panes[1].selected_agent_id.as_deref(),
        Some(id_b.as_str())
    );
}

#[test]
fn materialise_pane_lane_idempotent_on_repeat() {
    let mut state = fixture_state_with_two_backends();
    install_filtered(&mut state, None, 2, PathBuf::from("/work")).expect("ok");
    let lanes_before_first = state.agents.agents.len();

    let _ = materialise_pane_lane(&mut state, 0, "claude-haiku-4-5");
    let after_first = state.agents.agents.len();
    let _ = materialise_pane_lane(&mut state, 0, "claude-haiku-4-5");
    let after_second = state.agents.agents.len();

    assert_eq!(after_first, lanes_before_first + 1);
    assert_eq!(after_second, after_first);
}

#[test]
fn is_backend_family_recognises_closed_set_case_insensitive() {
    for fam in ["codex", "Claude", "GEMINI", "local"] {
        assert!(is_backend_family(fam), "{fam} should be a family");
    }
    for not_fam in ["claude-haiku-4-5", "gpt-5", "anthropic", ""] {
        assert!(
            !is_backend_family(not_fam),
            "{not_fam} should not be a family"
        );
    }
}
