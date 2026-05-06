use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::{AgentLaneKind, RosterTreeSelection};

/// Per-pane chat session anchored at its own working directory.
/// `selected_agent_id` drives mode: `Some` ⇒ chat for that lane, `None` ⇒
/// roster picker. `agent_id` retains the legacy `--backend <specific-id>`
/// pre-picked lane and stays empty otherwise until the operator commits.
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
    /// In-flight draft preserved while walking history with Up/Down.
    /// Lens-B aliases it to/from `AgentsState::chat_prompt_history_draft`
    /// per keystroke so per-pane history nav matches single-pane semantics.
    #[serde(default)]
    pub chat_prompt_history_draft: Option<String>,
    pub dir_search: Option<DirSearchState>,
    pub mission_id: Option<String>,
    /// Stable synthetic chat mission id (`mp-pane-NN-chat`) tagging this
    /// pane's chat messages so render filters and `@all` / abort helpers
    /// can scope per-pane. Recomputed from `pane_id` on load.
    #[serde(default, skip_serializing)]
    pub chat_mission_id: String,
    /// Swarm mission ids bound to this pane this session — used by
    /// `/abort all` to scope cancellation without scanning all missions.
    #[serde(default)]
    pub mission_ids: Vec<String>,
    /// Cursor row inside the per-pane roster while in roster mode. Survives
    /// focus changes (lives on the pane, not in any global state).
    #[serde(default)]
    pub roster_cursor: usize,
    /// Top visible row of the per-pane roster viewport. Lets the operator
    /// scroll a tall roster (Backend → Agent → Size leaves) inside a small
    /// pane. Clamped each render to `total_rows.saturating_sub(height)`.
    #[serde(default)]
    pub roster_scroll: usize,
    /// Per-pane set of agent ids whose Size/Role tree branches are
    /// collapsed. Pane-local so panes can show different folds without
    /// bleeding into each other or into Agent OPS.
    #[serde(skip)]
    pub roster_collapsed_agent_ids: HashSet<String>,
    /// Selected leaf inside the focused agent's Size/Role tree. `None`
    /// while the cursor is on a Backend or Agent row.
    #[serde(skip)]
    pub roster_tree_selected: Option<RosterTreeSelection>,
    /// Chat-thread scroll offset (separate from `chat_input_scroll`).
    /// Defaults to the `CONSOLE_SCROLL_BOTTOM` sentinel — wheel / PgUp /
    /// PgDn handlers MUST resolve it to `max_scroll` before applying a
    /// delta, otherwise scrolling up from bottom jumps to row 0.
    #[serde(default = "chat_thread_scroll_default")]
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
    /// Per-pane reasoning-effort overrides keyed by base agent id, read
    /// before falling back to `AgentsState` defaults. Roster click writes
    /// here only — global maps stay untouched until dispatch.
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

fn chat_thread_scroll_default() -> usize {
    crate::state::CONSOLE_SCROLL_BOTTOM
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
            chat_prompt_history_draft: None,
            dir_search: None,
            mission_id: None,
            chat_mission_id: String::new(),
            mission_ids: Vec::new(),
            roster_cursor: 0,
            roster_scroll: 0,
            roster_collapsed_agent_ids: HashSet::new(),
            roster_tree_selected: None,
            chat_thread_scroll: chat_thread_scroll_default(),
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

/// Directory-search overlay carried by an active pane. The runner
/// matches `generation` on inbound results so a stale walk can never
/// overwrite a newer query; `show_hidden` flips the `f` toggle from
/// the search bar.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DirSearchState {
    pub query: String,
    pub query_cursor: usize,
    pub results: Vec<PathBuf>,
    pub selected: usize,
    pub base: PathBuf,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub show_hidden: bool,
    /// Index of the first row rendered in the dropdown viewport.
    /// `selected - view_offset` is the visual row of the highlight.
    #[serde(default)]
    pub view_offset: usize,
    /// Last rendered visible row count (cached so move-helpers can
    /// clamp the viewport without access to the layout rect).
    #[serde(skip)]
    pub last_visible: u16,
    /// Bookmark set of paths the operator has expanded in browse mode
    /// (empty needle). The walker inlines one level of children for
    /// each path here so the renderer can show an in-place tree.
    #[serde(skip)]
    pub expanded: HashSet<PathBuf>,
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
    /// Multipane help overlay visibility. Toggled by F1 / `?` (chat
    /// mode, empty input). `#[serde(skip)]` because UI state should
    /// not survive a relaunch.
    #[serde(skip)]
    pub help_open: bool,
}

#[cfg(test)]
#[path = "../tests/multipane.rs"]
mod tests;
