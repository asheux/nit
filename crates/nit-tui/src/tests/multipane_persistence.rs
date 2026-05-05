use super::*;
use nit_core::PaneSession;
use std::path::PathBuf;

fn fixture_state() -> MultipaneState {
    MultipaneState {
        backend_agent_id: "claude-haiku-4-5".into(),
        panes: vec![
            PaneSession {
                pane_id: 0,
                cwd: PathBuf::from("/p0"),
                chat_input: "draft".into(),
                chat_input_cursor: 5,
                swarm_template: "lab".into(),
                swarm_mission: "auto".into(),
                has_run_mission: true,
                ..PaneSession::default()
            },
            PaneSession {
                pane_id: 1,
                cwd: PathBuf::from("/p1"),
                ..PaneSession::default()
            },
        ],
        focused: 1,
        grid_cols: 2,
        grid_rows: 1,
        backend_filter: Some("claude-haiku-4-5".into()),
        help_open: false,
    }
}

#[test]
fn save_then_load_roundtrips_per_pane_cwd_and_chat_input() {
    // Roundtrip through serde_json without touching the shared state
    // dir (ProjectDirs is unmocked in this crate's tests).
    let state = fixture_state();
    let json = serde_json::to_string_pretty(&state).unwrap();
    let loaded: MultipaneState = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.panes.len(), 2);
    assert_eq!(loaded.panes[0].cwd, PathBuf::from("/p0"));
    assert_eq!(loaded.panes[0].chat_input, "draft");
    assert_eq!(loaded.panes[0].chat_input_cursor, 5);
    assert_eq!(loaded.focused, 1);
    assert_eq!(loaded.backend_filter.as_deref(), Some("claude-haiku-4-5"));
    // help_open is #[serde(skip)] so always defaults.
    assert!(!loaded.help_open);
}

#[test]
fn merge_prior_lifts_per_pane_fields_when_panes_match() {
    let mut target = fixture_state();
    for p in &mut target.panes {
        p.chat_input.clear();
        p.cwd = PathBuf::from("/fresh");
        p.has_run_mission = false;
    }
    let prior = fixture_state();
    let merged = merge_prior(&mut target, prior);
    assert!(merged);
    assert_eq!(target.panes[0].cwd, PathBuf::from("/p0"));
    assert_eq!(target.panes[0].chat_input, "draft");
    assert_eq!(target.focused, 1);
}

#[test]
fn merge_prior_rejects_when_pane_count_changes() {
    let mut target = fixture_state();
    let mut prior = fixture_state();
    prior.panes.pop();
    let merged = merge_prior(&mut target, prior);
    assert!(!merged);
}

#[test]
fn is_fresh_flips_after_first_dispatch() {
    let mut state = fixture_state();
    for p in &mut state.panes {
        p.has_run_mission = false;
    }
    assert!(is_fresh(&state));
    state.panes[0].has_run_mission = true;
    assert!(!is_fresh(&state));
}

#[test]
fn workspace_hash_is_deterministic() {
    let p = PathBuf::from("/workspace/example");
    assert_eq!(workspace_hash(&p), workspace_hash(&p));
}
