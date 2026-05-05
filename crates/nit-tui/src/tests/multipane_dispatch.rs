use super::*;
use nit_core::{MultipaneState, PaneSession};
use std::path::PathBuf;

fn fixture_state() -> AppState {
    let buffer = nit_core::Buffer::empty("scratch", None);
    let notes = nit_core::Buffer::empty("notes", None);
    let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
    state.multipane = Some(MultipaneState {
        backend_agent_id: String::new(),
        panes: vec![
            PaneSession {
                pane_id: 0,
                cwd: PathBuf::from("/pane0"),
                ..PaneSession::default()
            },
            PaneSession {
                pane_id: 1,
                agent_id: "claude-haiku-4-5#mp-pane-01".into(),
                cwd: PathBuf::from("/pane1"),
                selected_agent_id: Some("claude-haiku-4-5#mp-pane-01".into()),
                ..PaneSession::default()
            },
        ],
        focused: 0,
        grid_cols: 2,
        grid_rows: 1,
        backend_filter: None,
        help_open: false,
    });
    state
}

#[test]
fn dispatch_no_selection_returns_marker_when_pane_unselected() {
    let mut state = fixture_state();
    let mut vitals = VitalsState::default();
    let outcome = dispatch_pane_prompt(&mut state, &mut vitals, None, None, 0, "hello".into());
    assert_eq!(outcome, DispatchOutcome::NoSelection);
}

#[test]
fn dispatch_unknown_pane_returns_marker() {
    let mut state = fixture_state();
    let mut vitals = VitalsState::default();
    let outcome = dispatch_pane_prompt(&mut state, &mut vitals, None, None, 99, "hello".into());
    assert_eq!(outcome, DispatchOutcome::PaneMissing);
}
