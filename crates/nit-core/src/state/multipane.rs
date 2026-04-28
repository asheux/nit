use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Per-pane chat session anchored at its own working directory. When a
/// pane has a chosen agent (`selected_agent_id == Some`), the pane renders
/// the chat thread for that lane and dispatch routes prompts to it. When
/// the field is `None` the pane shows a roster picker; the operator
/// commits a choice with Enter, which lazily allocates a per-pane lane
/// `<base>#mp-pane-NN` and stores its id back into `selected_agent_id`.
///
/// `agent_id` retains the legacy "pre-pick" lane id used by the
/// `--backend <specific-id>` flow (every pane lands in chat with that
/// lane already cloned). When `--backend` is omitted or names a family,
/// `agent_id` is empty until the operator commits a roster selection.
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
    /// Cursor row inside the per-pane roster while in roster mode. Survives
    /// focus changes (lives on the pane, not in any global state).
    #[serde(default)]
    pub roster_cursor: usize,
    /// Lane id chosen for this pane. `None` ⇒ render roster picker; `Some`
    /// ⇒ render chat for that lane (the lane is materialised lazily as
    /// `<base>#mp-pane-NN` on selection commit). Pre-picked at install
    /// when `--backend <specific-id>` is supplied.
    #[serde(default)]
    pub selected_agent_id: Option<String>,
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
///
/// `backend_agent_id` carries the operator's `--backend` argument verbatim
/// for diagnostics (empty when `--backend` was omitted). `backend_filter`
/// is the parsed scope: `None` ⇒ panes show the full roster and pick
/// independently; `Some(family)` ⇒ panes show only that family's lanes;
/// `Some(specific-id)` ⇒ install pre-picked every pane and `agent_id` is
/// already populated.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MultipaneState {
    pub backend_agent_id: String,
    pub panes: Vec<PaneSession>,
    pub focused: usize,
    pub grid_cols: usize,
    pub grid_rows: usize,
    /// Family alias or specific lane id from `--backend`. `None` ⇒ no
    /// filter (operator picks per pane).
    #[serde(default)]
    pub backend_filter: Option<String>,
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
            backend_filter: Some("test-model".into()),
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
    }

    #[test]
    fn multipane_default_has_no_backend_filter() {
        let mp = MultipaneState::default();
        assert!(mp.backend_filter.is_none());
        assert!(mp.panes.is_empty());
    }
}
