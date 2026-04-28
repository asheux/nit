use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::{AgentLaneKind, RosterTreeSelection};

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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Top visible row of the per-pane roster viewport. Lets the operator
    /// scroll a tall roster (Backend → Agent → Size leaves) inside a small
    /// pane. Clamped each render to `total_rows.saturating_sub(height)`.
    #[serde(default)]
    pub roster_scroll: usize,
    /// Backend groups expanded in this pane's roster. Pane-local so panes
    /// can show different families without bleeding selection into each
    /// other or into Agent OPS.
    #[serde(skip)]
    pub roster_expanded_backends: HashSet<AgentLaneKind>,
    /// Per-pane set of agent ids whose Size/Role tree branches are
    /// collapsed. Pane-local for the same reason as
    /// `roster_expanded_backends`.
    #[serde(skip)]
    pub roster_collapsed_agent_ids: HashSet<String>,
    /// Selected leaf inside the focused agent's Size/Role tree. `None`
    /// while the cursor is on a Backend or Agent row.
    #[serde(skip)]
    pub roster_tree_selected: Option<RosterTreeSelection>,
    /// Vertical scroll offset for the chat thread, separate from
    /// `chat_input_scroll` (which is reserved for input-box scrolling).
    /// Auto-sticks to bottom while zero; the wheel/keyboard handlers bump
    /// it upward and the renderer clamps to the max scroll each frame.
    #[serde(default)]
    pub chat_thread_scroll: usize,
    /// Lane id chosen for this pane. `None` ⇒ render roster picker; `Some`
    /// ⇒ render chat for that lane (the lane is materialised lazily as
    /// `<base>#mp-pane-NN` on selection commit). Pre-picked at install
    /// when `--backend <specific-id>` is supplied.
    #[serde(default)]
    pub selected_agent_id: Option<String>,
    /// Backend kind auto-expanded by the cursor's current position.
    /// Cleared and re-set on every cursor move so only one group is
    /// ever auto-expanded at a time.
    #[serde(skip)]
    pub auto_expanded_backend: Option<AgentLaneKind>,
    /// Agent id whose Size leaves are auto-expanded by the cursor's
    /// current position. Cleared when the cursor leaves the agent row.
    #[serde(skip)]
    pub auto_expanded_agent: Option<String>,
    /// Per-pane swarm template selection. Defaults to `"lab"`; seeded
    /// from `AgentsState::swarm_default_template` at pane construction.
    #[serde(default = "default_swarm_template")]
    pub swarm_template: String,
    /// Per-pane swarm mission selection. Defaults to `"auto"`; seeded
    /// from `AgentsState::swarm_default_mission` at pane construction.
    #[serde(default = "default_swarm_mission")]
    pub swarm_mission: String,
    /// Watermark flipped to `true` the first time this pane successfully
    /// dispatches a prompt. Gates the artifact-callout decoration in the
    /// pane's chat thread so a freshly-selected agent doesn't show a
    /// `(see ARTIFACTS)` link before any mission has run.
    #[serde(default)]
    pub has_run_mission: bool,
    /// Per-pane reasoning-effort overrides keyed by base agent id (e.g.
    /// `gpt-5`). Roster checkbox reads here first; falls back to the
    /// global `AgentsState` defaults so freshly-spawned panes still seed
    /// a sensible value. Writing through the per-pane roster click
    /// stores here only — the global maps are untouched until dispatch
    /// time.
    #[serde(default)]
    pub selected_effort: BTreeMap<String, String>,
    /// Active text selection inside the pane's chat thread. Coordinates
    /// are LOGICAL pane-thread row indices (pre-`chat_thread_scroll`).
    /// `#[serde(skip)]` because selections are ephemeral.
    #[serde(skip)]
    pub selection: Option<PaneSelection>,
}

/// Pane-local text selection. `anchor_*` is the mouse-down origin;
/// `end_*` follows the drag. Coordinates are pane-thread row indices
/// and column character offsets — independent of viewport scroll so
/// the highlight survives `chat_thread_scroll` changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSelection {
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

fn default_swarm_template() -> String {
    "lab".into()
}

fn default_swarm_mission() -> String {
    "auto".into()
}

impl Default for PaneSession {
    fn default() -> Self {
        Self {
            pane_id: 0,
            agent_id: String::new(),
            cwd: PathBuf::new(),
            chat_input: String::new(),
            chat_input_cursor: 0,
            chat_input_selection_anchor: None,
            chat_input_scroll: 0,
            chat_prompt_history: Vec::new(),
            chat_prompt_history_pos: None,
            dir_search: None,
            mission_id: None,
            roster_cursor: 0,
            roster_scroll: 0,
            roster_expanded_backends: HashSet::new(),
            roster_collapsed_agent_ids: HashSet::new(),
            roster_tree_selected: None,
            chat_thread_scroll: 0,
            selected_agent_id: None,
            auto_expanded_backend: None,
            auto_expanded_agent: None,
            swarm_template: default_swarm_template(),
            swarm_mission: default_swarm_mission(),
            has_run_mission: false,
            selected_effort: BTreeMap::new(),
            selection: None,
        }
    }
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
        assert_eq!(pane.roster_scroll, 0);
        assert!(pane.roster_expanded_backends.is_empty());
        assert!(pane.roster_collapsed_agent_ids.is_empty());
        assert!(pane.roster_tree_selected.is_none());
        assert_eq!(pane.chat_thread_scroll, 0);
        assert!(pane.auto_expanded_backend.is_none());
        assert!(pane.auto_expanded_agent.is_none());
        assert!(!pane.has_run_mission);
        assert!(pane.selected_effort.is_empty());
        assert!(pane.selection.is_none());
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
}
