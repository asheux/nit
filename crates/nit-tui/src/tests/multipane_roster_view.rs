use super::*;
use nit_core::{AgentLane, AgentLaneKind, AgentStatus, AgentsState, AppState, Buffer};
use std::path::PathBuf;

fn lane(id: &str, lane_label: &str, kind: AgentLaneKind) -> AgentLane {
    AgentLane {
        id: id.into(),
        role: id.into(),
        lane: lane_label.into(),
        kind,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    }
}

fn shadow_lane(id: &str, kind: AgentLaneKind) -> AgentLane {
    let mut l = lane(id, "shadow", kind);
    l.shadow = true;
    l
}

fn fixture_state() -> AppState {
    let buffer = Buffer::empty("scratch", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
    state.agents = AgentsState::default();
    state.agents.agents = vec![
        lane("claude-haiku-4-5", "Claude", AgentLaneKind::Claude),
        lane("claude-opus-4-6", "Claude", AgentLaneKind::Claude),
        lane("gpt-5", "Codex", AgentLaneKind::Codex),
        lane("gemini-2.5-pro", "Gemini", AgentLaneKind::Gemini),
        lane("local", "Local", AgentLaneKind::Mock),
        shadow_lane("claude-haiku-4-5#shadow-x", AgentLaneKind::Claude),
        lane(
            "claude-haiku-4-5#mp-pane-00",
            "Claude",
            AgentLaneKind::Claude,
        ),
    ];
    state.agents.codex_supported_reasoning_efforts.insert(
        "gpt-5".into(),
        vec!["low".into(), "medium".into(), "high".into()],
    );
    state
        .agents
        .codex_selected_reasoning_effort
        .insert("gpt-5".into(), "medium".into());
    state.agents.claude_supported_efforts.insert(
        "claude-haiku-4-5".into(),
        vec!["low".into(), "medium".into(), "high".into(), "max".into()],
    );
    state.multipane = Some(nit_core::MultipaneState {
        backend_agent_id: String::new(),
        panes: vec![PaneSession::default(), PaneSession::default()],
        focused: 0,
        grid_cols: 2,
        grid_rows: 1,
        backend_filter: None,
        help_open: false,
    });
    state
}

fn pane_auto_expanded(kind: AgentLaneKind, agent_id: &str) -> PaneSession {
    PaneSession {
        auto_expanded_backend: Some(kind),
        auto_expanded_agent: Some(agent_id.to_string()),
        ..PaneSession::default()
    }
}

fn pane_auto_backend(kind: AgentLaneKind) -> PaneSession {
    PaneSession {
        auto_expanded_backend: Some(kind),
        ..PaneSession::default()
    }
}

#[test]
fn compute_rows_starts_with_template_mission_spacer() {
    let state = fixture_state();
    let pane = PaneSession::default();
    let rows = compute_rows(&state, &pane, None);
    assert!(matches!(rows[0], PaneRosterRow::Template));
    assert!(matches!(rows[1], PaneRosterRow::Mission));
    assert!(matches!(rows[2], PaneRosterRow::Spacer));
}

#[test]
fn compute_rows_collapsed_backends_show_only_headers() {
    let state = fixture_state();
    let pane = PaneSession::default();
    let rows = compute_rows(&state, &pane, None);
    let agent_rows = rows
        .iter()
        .filter(|r| matches!(r, PaneRosterRow::Agent { .. }))
        .count();
    assert_eq!(agent_rows, 0, "no agents until backend expanded");
    let backend_rows = rows
        .iter()
        .filter(|r| matches!(r, PaneRosterRow::Backend { .. }))
        .count();
    assert_eq!(backend_rows, 4, "Codex Claude Gemini Local visible");
}

#[test]
fn compute_rows_expand_codex_shows_lanes_and_size_leaves() {
    let state = fixture_state();
    // Auto-expand both the Codex backend and the gpt-5 agent so size
    // leaves render under the new cursor-driven semantics.
    let pane = pane_auto_expanded(AgentLaneKind::Codex, "gpt-5");
    let rows = compute_rows(&state, &pane, None);
    let agent_ids: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            PaneRosterRow::Agent { agent_id, .. } => Some(agent_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(agent_ids, ["gpt-5"], "only Codex group expanded");

    let size_leaves: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            PaneRosterRow::SizeLeaf { effort, .. } => Some(effort.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(size_leaves, ["low", "medium", "high"]);

    let checked: Vec<bool> = rows
        .iter()
        .filter_map(|r| match r {
            PaneRosterRow::SizeLeaf { checked, .. } => Some(*checked),
            _ => None,
        })
        .collect();
    assert_eq!(checked, [false, true, false], "medium is the chosen effort");
}

#[test]
fn compute_rows_auto_expanded_backend_alone_hides_size_leaves() {
    let state = fixture_state();
    let pane = PaneSession {
        auto_expanded_backend: Some(AgentLaneKind::Codex),
        ..PaneSession::default()
    };
    let rows = compute_rows(&state, &pane, None);
    let agent_count = rows
        .iter()
        .filter(|r| matches!(r, PaneRosterRow::Agent { .. }))
        .count();
    assert_eq!(agent_count, 1, "agents render when backend auto-expanded");
    let leaf_count = rows
        .iter()
        .filter(|r| matches!(r, PaneRosterRow::SizeLeaf { .. }))
        .count();
    assert_eq!(
        leaf_count, 0,
        "size leaves stay hidden until the agent itself is auto-expanded"
    );
}

#[test]
fn compute_rows_collapsed_agent_skips_size_leaves() {
    let state = fixture_state();
    // Cursor on the agent (auto_expanded_agent latches), but operator
    // explicitly collapsed the Size branch — leaves must stay hidden.
    let mut pane = pane_auto_expanded(AgentLaneKind::Codex, "gpt-5");
    pane.roster_collapsed_agent_ids.insert("gpt-5".into());
    let rows = compute_rows(&state, &pane, None);
    let leaf_count = rows
        .iter()
        .filter(|r| matches!(r, PaneRosterRow::SizeLeaf { .. }))
        .count();
    assert_eq!(leaf_count, 0);
}

#[test]
fn selectable_count_excludes_template_mission_spacer_empty() {
    let state = fixture_state();
    let pane = PaneSession::default();
    let rows = compute_rows(&state, &pane, None);
    // 4 backend rows (Codex Claude Gemini Local), no agent expand.
    assert_eq!(selectable_count(&rows), 4);
}

#[test]
fn cursor_for_row_index_skips_non_selectable() {
    let state = fixture_state();
    let pane = pane_auto_backend(AgentLaneKind::Codex);
    let rows = compute_rows(&state, &pane, None);
    let target_idx = rows
        .iter()
        .position(|r| matches!(r, PaneRosterRow::Agent { agent_id, .. } if agent_id == "gpt-5"))
        .expect("gpt-5 row");
    let cursor = cursor_for_row_index(&rows, target_idx).expect("cursor");
    // Skipping Template, Mission, Spacer (3 leading rows) and Backend Codex (1 backend),
    // gpt-5 is the 2nd selectable row → cursor index 1.
    assert_eq!(cursor, 1);
}

#[test]
fn template_word_at_x_resolves_offset() {
    // " Template:  lab   parallel   bulk " — first word " lab " starts after prefix.
    let prefix_len = TEMPLATE_LABEL.chars().count();
    let lab_start = prefix_len;
    assert_eq!(template_word_at_x(lab_start), Some("lab"));
    assert_eq!(template_word_at_x(lab_start + 1), Some("lab"));
    // The selectable token includes pad spaces (" lab "), so the trailing space is still a hit
    let lab_token_end = prefix_len + " lab ".chars().count();
    assert_eq!(template_word_at_x(lab_token_end - 1), Some("lab"));
    // After the single-space separator we land on " parallel "
    let parallel_start = lab_token_end + 1;
    assert_eq!(template_word_at_x(parallel_start), Some("parallel"));
    assert_eq!(template_word_at_x(parallel_start + 5), Some("parallel"));
}

#[test]
fn mission_word_at_x_resolves_computational() {
    let prefix_len = MISSION_LABEL.chars().count();
    let auto_token_len = " auto ".chars().count();
    let general_token_len = " general ".chars().count();
    let research_token_len = " research ".chars().count();
    // After auto + sep + general + sep + research + sep we land on " computational "
    let comp_start =
        prefix_len + auto_token_len + 1 + general_token_len + 1 + research_token_len + 1;
    assert_eq!(
        mission_word_at_x(comp_start),
        Some("computational-research")
    );
}

#[test]
fn toggle_size_leaf_writes_to_codex_selected_effort() {
    let mut state = fixture_state();
    let toggled = toggle_size_leaf(&mut state, 0, "gpt-5", 2);
    assert!(toggled);
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[0]
            .selected_effort
            .get("gpt-5"),
        Some(&"high".to_string())
    );
    assert_eq!(
        state.agents.codex_selected_reasoning_effort.get("gpt-5"),
        Some(&"medium".to_string()),
        "global default seeded by fixture must stay untouched"
    );
}

#[test]
fn toggle_size_leaf_writes_to_claude_selected_effort() {
    let mut state = fixture_state();
    let toggled = toggle_size_leaf(&mut state, 0, "claude-haiku-4-5", 3);
    assert!(toggled);
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[0]
            .selected_effort
            .get("claude-haiku-4-5"),
        Some(&"max".to_string())
    );
    assert!(
        !state
            .agents
            .claude_selected_effort
            .contains_key("claude-haiku-4-5"),
        "global claude_selected_effort must stay untouched"
    );
}

#[test]
fn two_panes_pick_independent_sizes() {
    let mut state = fixture_state();
    assert!(toggle_size_leaf(&mut state, 0, "gpt-5", 0));
    assert!(toggle_size_leaf(&mut state, 1, "gpt-5", 2));
    let panes = &state.multipane.as_ref().unwrap().panes;
    assert_eq!(panes[0].selected_effort.get("gpt-5"), Some(&"low".into()));
    assert_eq!(panes[1].selected_effort.get("gpt-5"), Some(&"high".into()));
}

#[test]
fn toggle_agent_tree_collapse_round_trips() {
    let mut pane = PaneSession::default();
    toggle_agent_tree_collapse(&mut pane, "gpt-5");
    assert!(pane.roster_collapsed_agent_ids.contains("gpt-5"));
    toggle_agent_tree_collapse(&mut pane, "gpt-5");
    assert!(!pane.roster_collapsed_agent_ids.contains("gpt-5"));
}

#[test]
fn sync_tree_selection_sets_size_leaf() {
    let mut pane = PaneSession::default();
    let row = PaneRosterRow::SizeLeaf {
        agent_id: "gpt-5".into(),
        leaf_idx: 1,
        effort: "medium".into(),
        checked: true,
    };
    sync_tree_selection(&mut pane, Some(&row));
    assert_eq!(
        pane.roster_tree_selected,
        Some(RosterTreeSelection {
            branch: RosterTreeBranch::Size,
            leaf_idx: 1,
        })
    );

    sync_tree_selection(
        &mut pane,
        Some(&PaneRosterRow::Backend {
            kind: AgentLaneKind::Codex,
        }),
    );
    assert!(pane.roster_tree_selected.is_none());
}

#[test]
fn family_to_kind_resolves_closed_set() {
    assert_eq!(family_to_kind("codex"), Some(AgentLaneKind::Codex));
    assert_eq!(family_to_kind("CLAUDE"), Some(AgentLaneKind::Claude));
    assert_eq!(family_to_kind("Gemini"), Some(AgentLaneKind::Gemini));
    assert_eq!(family_to_kind("local"), Some(AgentLaneKind::Mock));
    assert_eq!(family_to_kind("anthropic"), None);
}

#[test]
fn empty_state_text_uses_filter_label() {
    assert!(empty_state_text(Some("claude")).contains("Claude"));
    assert!(empty_state_text(None).contains("agent"));
}
