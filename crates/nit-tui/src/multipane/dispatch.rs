use nit_core::AppState;

use crate::app::dispatch_agent_prompt;
use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::vitals::VitalsState;

/// Outcome of `dispatch_pane_prompt`. The roster runtime turns
/// `NoSelection` into a one-line system message in the pane's chat
/// history rather than silently dropping the prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DispatchOutcome {
    Dispatched,
    NoSelection,
    PaneMissing,
}

/// Dispatch the focused-pane prompt through the standard
/// `dispatch_agent_prompt` path. The pane's `cwd` is read at the dispatch
/// leaf via `app::dispatch::resolve_dispatch_cwd`, so this wrapper has no
/// `cwd` parameter — queue dequeues stay correct because the leaf
/// resolves at dispatch time, not enqueue time.
///
/// When the pane has no committed selection (`selected_agent_id == None`
/// and `agent_id` empty), returns `NoSelection` without dispatching so
/// the caller can post a "no agent selected" notice.
pub(crate) fn dispatch_pane_prompt(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    pane_idx: usize,
    prompt: String,
) -> DispatchOutcome {
    let Some(mp) = state.multipane.as_ref() else {
        return DispatchOutcome::PaneMissing;
    };
    let Some(pane) = mp.panes.get(pane_idx) else {
        return DispatchOutcome::PaneMissing;
    };
    let agent_id = pane
        .selected_agent_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(pane.agent_id.clone()).filter(|s| !s.is_empty()));
    let Some(agent_id) = agent_id else {
        return DispatchOutcome::NoSelection;
    };
    let mission_id = pane.mission_id.clone();
    dispatch_agent_prompt(state, vitals, codex, claude, agent_id, mission_id, prompt);
    DispatchOutcome::Dispatched
}

#[cfg(test)]
mod tests {
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
}
