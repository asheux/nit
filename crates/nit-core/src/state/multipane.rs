use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Per-pane chat session anchored at its own working directory. Each pane
/// has a unique `agent_id` (e.g. `claude-haiku-4-5#mp-pane-03`) so messages
/// in `state.agents.messages` filter naturally per pane without a
/// duplicate Vec.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PaneSession {
    pub pane_id: usize,
    pub agent_id: String,
    pub cwd: PathBuf,
    pub chat_input: String,
    pub chat_input_cursor: usize,
    pub chat_input_selection_anchor: Option<usize>,
    pub chat_input_scroll: usize,
    pub chat_prompt_history: Vec<String>,
    pub chat_prompt_history_pos: Option<usize>,
    // TODO(multipane phase 4): dir search
    pub dir_search: Option<DirSearchState>,
    pub mission_id: Option<String>,
}

/// Directory-search overlay carried by an active pane. Stubbed for Phase
/// 1–3; populated and rendered in Phase 4.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DirSearchState {
    pub query: String,
    pub query_cursor: usize,
    pub results: Vec<PathBuf>,
    pub selected: usize,
    pub base: PathBuf,
}

/// Top-level state for the multipane launch mode. When `AppState.multipane`
/// is `Some`, the standard single-pane `app::runner::run_loop` is never
/// entered — the multipane event loop owns rendering and input.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MultipaneState {
    pub backend_agent_id: String,
    pub panes: Vec<PaneSession>,
    pub focused: usize,
    pub grid_cols: usize,
    pub grid_rows: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
