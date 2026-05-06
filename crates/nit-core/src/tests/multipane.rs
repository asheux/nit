use super::*;
use std::path::PathBuf;

#[test]
fn multipane_state_isolates_chat_inputs() {
    let mut mp = MultipaneState {
        backend_agent_id: "test-model".into(),
        panes: (0..3)
            .map(|i| PaneSession {
                pane_id: i,
                agent_id: format!("test-model#mp-pane-{i:02}"),
                cwd: PathBuf::from("/tmp"),
                ..PaneSession::default()
            })
            .collect(),
        focused: 0,
        grid_cols: 2,
        grid_rows: 2,
        backend_filter: Some("test-model".into()),
        help_open: false,
    };

    mp.panes[1].chat_input = "hello pane 1".into();
    mp.panes[1].chat_input_cursor = 5;

    assert_eq!(mp.panes[0].chat_input, "");
    assert_eq!(mp.panes[2].chat_input, "");
    assert_eq!(mp.panes[1].chat_input, "hello pane 1");
    assert_eq!(mp.panes[1].chat_input_cursor, 5);
}

#[test]
fn pane_session_default_has_no_dir_search() {
    let pane = PaneSession::default();
    assert!(pane.dir_search.is_none());
    assert!(pane.mission_id.is_none());
    assert!(pane.chat_prompt_history.is_empty());
    assert!(pane.selected_agent_id.is_none());
    assert_eq!(pane.roster_cursor, 0);
    assert_eq!(pane.roster_scroll, 0);
    assert!(pane.roster_collapsed_agent_ids.is_empty());
    assert!(pane.roster_tree_selected.is_none());
    // The "stick to bottom" sentinel — newly created panes follow new
    // chat content automatically.
    assert_eq!(pane.chat_thread_scroll, crate::state::CONSOLE_SCROLL_BOTTOM);
    assert!(pane.auto_expanded_backend.is_none());
    assert!(pane.auto_expanded_agent.is_none());
    assert!(!pane.has_run_mission);
    assert!(pane.selected_effort.is_empty());
    assert!(pane.selection.is_none());
}

#[test]
fn dir_search_state_default_zeroes_generation_and_hidden() {
    let s = DirSearchState::default();
    assert_eq!(s.generation, 0);
    assert!(!s.show_hidden);
    assert!(s.query.is_empty());
    assert_eq!(s.selected, 0);
    assert!(s.results.is_empty());
    assert_eq!(s.view_offset, 0);
    assert_eq!(s.last_visible, 0);
    assert!(s.expanded.is_empty());
}

#[test]
fn dir_search_state_serde_skips_expanded_and_last_visible() {
    let mut s = DirSearchState {
        query: "foo".into(),
        view_offset: 7,
        last_visible: 12,
        ..Default::default()
    };
    s.expanded.insert(PathBuf::from("/tmp/a"));
    s.expanded.insert(PathBuf::from("/tmp/b"));
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(!json.contains("expanded"), "expanded must be skipped");
    assert!(
        !json.contains("last_visible"),
        "last_visible must be skipped"
    );
    assert!(json.contains("view_offset"), "view_offset must persist");
    let round: DirSearchState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(round.view_offset, 7);
    assert_eq!(round.last_visible, 0, "last_visible defaults on load");
    assert!(round.expanded.is_empty(), "expanded defaults on load");
    assert_eq!(round.query, "foo");
}

#[test]
fn pane_session_default_template_is_lab() {
    let pane = PaneSession::default();
    assert_eq!(pane.swarm_template, "lab");
}

#[test]
fn pane_session_default_mission_is_auto() {
    let pane = PaneSession::default();
    assert_eq!(pane.swarm_mission, "auto");
}

#[test]
fn multipane_default_has_no_backend_filter() {
    let mp = MultipaneState::default();
    assert!(mp.backend_filter.is_none());
    assert!(mp.panes.is_empty());
}
