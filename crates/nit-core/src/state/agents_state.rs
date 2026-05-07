use std::collections::HashMap;

use super::*;

/// Initial MCP status before any connection attempt: offline with the
/// "mock://offline" placeholder endpoint that the CLI overrides on launch.
fn default_offline_mcp_status() -> McpStatus {
    McpStatus {
        state: McpConnectionState::Disconnected,
        endpoint: "mock://offline".into(),
        latency_ms: None,
        last_error: None,
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentsState {
    pub selected_agent: Option<String>,
    pub selected_mission: Option<String>,
    pub dock_tab: AgentOpsTab,
    pub console_tab: AgentConsoleTab,
    pub chat_input: String,
    #[serde(default)]
    pub chat_input_cursor: usize,
    /// Selection anchor in the Agent Chat compose box (char index in `chat_input`).
    /// Runtime-only.
    #[serde(skip)]
    pub chat_input_selection_anchor: Option<usize>,
    #[serde(skip, default = "chat_input_scroll_default")]
    pub chat_input_scroll: usize,
    /// Previously submitted prompts from the Agent Chat compose box.
    /// Runtime-only: avoid persisting operator prompts to disk.
    #[serde(skip)]
    pub chat_prompt_history: Vec<String>,
    /// History navigation cursor (index into `chat_prompt_history`) while cycling via Up/Down.
    /// Runtime-only.
    #[serde(skip)]
    pub chat_prompt_history_pos: Option<usize>,
    /// Draft compose text captured when entering history navigation.
    /// Runtime-only.
    #[serde(skip)]
    pub chat_prompt_history_draft: Option<String>,
    pub chat_channel: AgentChannel,
    /// Default swarm template used when auto-detecting swarm prompts or when `@swarm` omits an
    /// explicit `template=...` argument.
    #[serde(default = "swarm_default_template_default")]
    pub swarm_default_template: String,
    /// Default swarm mission preset used when swarm prompts omit an explicit mission focus.
    /// `auto` preserves prompt-based detection; other values pin the mission kind.
    #[serde(default = "swarm_default_mission_default")]
    pub swarm_default_mission: String,
    /// Per-agent role hint for swarm planning (used by `parallel`/`bulk`).
    /// Missing entry means "all roles".
    #[serde(default)]
    pub swarm_role_by_agent_id: HashMap<String, String>,
    /// Agents marked as priority in the roster (used as a planning hint for `parallel`/`bulk`).
    #[serde(default)]
    pub swarm_priority_agent_ids: HashSet<String>,
    /// Maximum number of Codex turns to run concurrently (from `--codex-max-parallel-turns`).
    /// Runtime-only; used as a hint for auto-starting swarm sizes.
    #[serde(skip, default = "codex_max_parallel_turns_default")]
    pub codex_max_parallel_turns: usize,
    /// Maximum number of Claude turns to run concurrently.
    /// Runtime-only; used as a hint for auto-starting swarm sizes.
    #[serde(skip, default = "claude_max_parallel_turns_default")]
    pub claude_max_parallel_turns: usize,
    pub agents: Vec<AgentLane>,
    /// Cache of `agents[i].id -> i` so per-event handlers and per-frame
    /// renderers can look an agent up in O(1) instead of scanning the
    /// roster vec. Maintained explicitly when the production write paths
    /// (`agent_bus::upsert_agent`, `swarm::clones`, `multipane::setup`)
    /// mutate the vec; lookups via `agents_get` / `agents_get_mut`
    /// fall back to a linear scan if the entry is stale, so test code
    /// doing `agents.push` directly stays correct.
    #[serde(skip, default)]
    pub agents_index: HashMap<String, usize>,
    pub missions: Vec<MissionRecord>,
    pub patches: Vec<PatchProposal>,
    pub messages: Vec<AgentMessage>,
    pub evidence: Vec<EvidenceItem>,
    pub alerts: Vec<AgentAlert>,
    pub diag_events: Vec<AgentDiagnosticEvent>,
    pub mcp: McpStatus,
    pub roster_selected: usize,
    /// Selected backend group row in the roster, when keyboard/mouse focus is on a backend
    /// instead of a concrete model row.
    /// Runtime-only; UI navigation state.
    #[serde(skip)]
    pub roster_selected_backend: Option<AgentLaneKind>,
    /// When selecting in the roster "tree" under the selected model, this stores the selected
    /// child-row (Size/Role) leaf index for keyboard navigation.
    /// Runtime-only; UI navigation state.
    #[serde(skip)]
    pub roster_tree_selected: Option<RosterTreeSelection>,
    /// Backends whose model rows are expanded in the roster.
    /// Runtime-only; UI navigation state.
    #[serde(skip, default)]
    pub roster_expanded_backend_kinds: HashSet<AgentLaneKind>,
    /// Agents whose roster tree (Size/Role) is collapsed.
    /// Runtime-only; UI navigation state.
    #[serde(skip, default)]
    pub roster_tree_collapsed_agent_ids: HashSet<String>,
    pub mission_selected: usize,
    pub alert_selected: usize,
    pub patch_selected: usize,
    #[serde(skip)]
    pub artifacts_selected: usize,
    /// Indices of PROMPT cards whose children are collapsed in the artifact tree.
    #[serde(skip, default)]
    pub artifacts_collapsed_prompts: HashSet<usize>,
    #[serde(skip)]
    pub artifacts_popup_open: bool,
    #[serde(skip)]
    pub artifacts_popup_scroll: usize,
    /// Last max scroll computed during render. Cached so wheel/keyboard scroll
    /// handlers can clamp without paying the cost of rebuilding the rendered
    /// markdown (`build_lines`) on every input event. Updated on each render,
    /// starts at `usize::MAX` so the first scroll before any render is unclamped.
    /// Runtime-only.
    #[serde(skip, default = "artifacts_popup_last_max_scroll_default")]
    pub artifacts_popup_last_max_scroll: usize,
    /// Chat input text for the artifacts popup compose box. Runtime-only.
    #[serde(skip)]
    pub artifacts_popup_chat_input: String,
    /// Cursor position (char index) in the artifacts popup compose box. Runtime-only.
    #[serde(skip)]
    pub artifacts_popup_chat_cursor: usize,
    /// Selection anchor in the artifacts popup compose box. Runtime-only.
    #[serde(skip)]
    pub artifacts_popup_chat_selection_anchor: Option<usize>,
    /// Scroll offset for the artifacts popup compose box. Runtime-only.
    #[serde(skip, default = "chat_input_scroll_default")]
    pub artifacts_popup_chat_scroll: usize,
    #[serde(skip)]
    pub artifacts_history_popup_open: bool,
    #[serde(skip)]
    pub artifacts_history_popup_scroll: usize,
    #[serde(skip)]
    pub artifacts_history_selected: usize,
    #[serde(skip)]
    pub artifacts_selected_saved_run_path: Option<String>,
    #[serde(skip, default)]
    pub artifacts_history_filter: SavedRunHistoryFilter,
    #[serde(skip)]
    pub artifacts_history_pending_action: Option<SavedRunHistoryPendingAction>,
    // --- Global Archive Browser ---
    #[serde(skip)]
    pub global_archive_open: bool,
    #[serde(skip)]
    pub global_archive_query: String,
    #[serde(skip)]
    pub global_archive_query_cursor: usize,
    #[serde(skip)]
    pub global_archive_selected: usize,
    #[serde(skip)]
    pub global_archive_scroll: usize,
    #[serde(skip, default)]
    pub global_archive_filter: SavedRunHistoryFilter,
    /// Full index of all archive entries (built when popup opens).
    #[serde(skip)]
    pub global_archive_index: Vec<GlobalArchiveEntry>,
    /// Filtered results: (score, index_into_global_archive_index).
    #[serde(skip)]
    pub global_archive_filtered: Vec<(i64, usize)>,
    /// When an artifact is opened from the archive, this holds the entry so the
    /// artifact popup can load the correct content directly from the run.json
    /// instead of relying on card-index matching.
    #[serde(skip)]
    pub global_archive_opened_entry: Option<GlobalArchiveEntry>,
    #[serde(skip)]
    pub ops_scroll: usize,
    /// Job output viewport dimensions (Agent Ops body area). Runtime-only.
    #[serde(skip)]
    pub ops_viewport_width: usize,
    /// Job output viewport dimensions (Agent Ops body area). Runtime-only.
    #[serde(skip)]
    pub ops_viewport_height: usize,
    /// Scroll offset for the agent console thread view.
    /// `CONSOLE_SCROLL_BOTTOM` (usize::MAX) means "auto-scroll to bottom".
    #[serde(skip)]
    pub console_scroll: usize,
    /// Cached max scroll from last render (used by input handlers to clamp).
    #[serde(skip)]
    pub console_max_scroll: usize,
    #[serde(skip)]
    pub console_rows_cache: AgentConsoleRowsCache,
    #[serde(skip)]
    pub event_epoch: u64,
    #[serde(skip)]
    pub pending_provenance_mission_ids: Vec<String>,
    #[serde(skip)]
    pub pending_provenance_agent_ids: Vec<String>,
    #[serde(skip)]
    pub pending_legacy_notes_alert: Option<String>,
    /// Codex model metadata (effective context window tokens) keyed by model slug.
    /// Runtime-only; populated when seeding the roster from `~/.codex/models_cache.json`.
    #[serde(skip)]
    pub codex_effective_context_window_tokens: HashMap<String, u32>,
    /// Best-effort estimated context tokens used per mission thread (heuristic; used for UI).
    /// Runtime-only.
    #[serde(skip)]
    pub codex_estimated_tokens_used_by_mission: HashMap<String, u32>,
    /// Codex model default reasoning effort (e.g. low/medium/high/xhigh) keyed by model slug.
    /// Runtime-only; populated when seeding the roster from `~/.codex/models_cache.json`.
    #[serde(skip)]
    pub codex_default_reasoning_effort: HashMap<String, String>,
    /// Codex model supported reasoning effort "sizes" keyed by model slug.
    /// Runtime-only; populated when seeding the roster from `~/.codex/models_cache.json`.
    #[serde(skip)]
    pub codex_supported_reasoning_efforts: HashMap<String, Vec<String>>,
    /// Codex model operator-selected reasoning effort keyed by model slug.
    /// Runtime-only; defaults to `codex_default_reasoning_effort` but can be changed in the roster.
    #[serde(skip)]
    pub codex_selected_reasoning_effort: HashMap<String, String>,
    /// Best-effort context remaining percentage for the currently running Codex turn.
    /// Runtime-only; updated when dispatching a Codex turn.
    #[serde(skip)]
    pub codex_context_remaining_pct: HashMap<String, u8>,
    /// Last-known Codex context remaining percentage for a mission thread, keyed by mission id then
    /// model slug.
    /// Runtime-only; updated when Codex reports token counts for a mission-backed session.
    #[serde(skip)]
    pub codex_mission_context_remaining_pct: HashMap<String, HashMap<String, u8>>,
    /// Last-known Codex total token usage (non-mission chat), keyed by model slug.
    /// Runtime-only; updated when Codex reports token counts.
    #[serde(skip)]
    pub codex_used_tokens: HashMap<String, u32>,
    /// Last-known Codex total token usage for mission threads, keyed by mission id then model slug.
    /// Runtime-only; updated when Codex reports token counts.
    #[serde(skip)]
    pub codex_mission_used_tokens: HashMap<String, HashMap<String, u32>>,
    /// Maps agent_id → index of the user prompt message that triggered the current turn.
    /// Set at dispatch time, consumed by TurnCompleted to link responses to their prompts.
    /// Runtime-only.
    #[serde(skip)]
    pub codex_turn_prompt_idx: HashMap<String, usize>,
    /// Codex session/thread ids keyed by model slug for non-mission chat. Used to resume an
    /// ad-hoc "agent chat" thread across multiple prompts without requiring a mission.
    /// Runtime-only.
    #[serde(skip)]
    pub codex_thread_ids: HashMap<String, String>,
    /// Codex session/thread ids keyed by mission id. Used to resume a "live mission" thread across
    /// multiple prompts without prompt-stitching.
    /// Runtime-only.
    #[serde(skip)]
    pub codex_mission_thread_ids: HashMap<String, HashMap<String, String>>,
    /// Active backend turn telemetry keyed by agent id.
    /// Runtime-only.
    #[serde(skip)]
    pub active_turns: HashMap<String, AgentTurnState>,
    /// Codex turns queued by the operator while another Codex turn is still running.
    /// Runtime-only.
    #[serde(skip)]
    pub queued_codex_turns: VecDeque<QueuedCodexTurn>,
    /// Workspace-wide genome scan progress: `Some((done, total))` while the
    /// scan is in flight, `None` when idle. Updated from the runtime each
    /// main-loop tick; read by the agent-console breather to render the
    /// "Evaluating genome: X/Y files" indicator.
    /// Runtime-only.
    #[serde(skip)]
    pub workspace_scan_progress: Option<(usize, usize)>,
    /// Whether the `codex` CLI is available in PATH (used for backend inventory in the roster UI).
    /// Runtime-only.
    #[serde(skip)]
    pub codex_cli_available: bool,
    /// Whether the `claude` CLI is available in PATH (used for backend inventory in the roster UI).
    /// Runtime-only.
    #[serde(skip)]
    pub claude_cli_available: bool,
    /// Whether the `gemini` CLI is available in PATH (used for backend inventory in the roster UI).
    /// Runtime-only.
    #[serde(skip)]
    pub gemini_cli_available: bool,
    /// Claude models discovered from the backend (CLI/API). Runtime-only.
    #[serde(skip)]
    pub claude_models: Vec<String>,
    /// Claude model discovery error (if any). Runtime-only.
    #[serde(skip)]
    pub claude_models_error: Option<String>,
    /// Claude model metadata (effective context window tokens) keyed by model slug.
    /// Runtime-only; populated when seeding the roster from probed Claude models.
    #[serde(skip)]
    pub claude_effective_context_window_tokens: HashMap<String, u32>,
    /// Best-effort estimated context tokens used per mission session (heuristic; used for UI).
    /// Runtime-only.
    #[serde(skip)]
    pub claude_estimated_tokens_used_by_mission: HashMap<String, u32>,
    /// Claude model default effort level (e.g. low/medium/high/max) keyed by model slug.
    /// Runtime-only; populated when seeding the roster from probed Claude models.
    #[serde(skip)]
    pub claude_default_effort: HashMap<String, String>,
    /// Claude model supported effort levels keyed by model slug.
    /// Runtime-only; populated when seeding the roster from probed Claude models.
    #[serde(skip)]
    pub claude_supported_efforts: HashMap<String, Vec<String>>,
    /// Claude model operator-selected effort level keyed by model slug.
    /// Runtime-only; defaults to `claude_default_effort` but can be changed in the roster.
    #[serde(skip)]
    pub claude_selected_effort: HashMap<String, String>,
    /// Best-effort context remaining percentage for the currently running Claude turn.
    /// Runtime-only; updated when dispatching a Claude turn.
    #[serde(skip)]
    pub claude_context_remaining_pct: HashMap<String, u8>,
    /// Last-known Claude context remaining percentage for a mission session, keyed by mission id
    /// then model slug.
    /// Runtime-only; updated when Claude reports token counts for a mission-backed session.
    #[serde(skip)]
    pub claude_mission_context_remaining_pct: HashMap<String, HashMap<String, u8>>,
    /// Last-known Claude total token usage (non-mission chat), keyed by model slug.
    /// Runtime-only; updated when Claude reports token counts.
    #[serde(skip)]
    pub claude_used_tokens: HashMap<String, u32>,
    /// Last-known Claude total token usage for mission sessions, keyed by mission id then model
    /// slug. Runtime-only; updated when Claude reports token counts.
    #[serde(skip)]
    pub claude_mission_used_tokens: HashMap<String, HashMap<String, u32>>,
    /// Maps agent_id → index of the user prompt message that triggered the current Claude turn.
    /// Set at dispatch time, consumed by TurnCompleted to link responses to their prompts.
    /// Runtime-only.
    #[serde(skip)]
    pub claude_turn_prompt_idx: HashMap<String, usize>,
    /// Claude session ids keyed by model slug for non-mission chat. Used to resume an ad-hoc
    /// "agent chat" session across multiple prompts without requiring a mission.
    /// Runtime-only.
    #[serde(skip)]
    pub claude_session_ids: HashMap<String, String>,
    /// Claude session ids keyed by mission id then model slug. Used to resume a "live mission"
    /// session across multiple prompts.
    /// Runtime-only.
    #[serde(skip)]
    pub claude_mission_session_ids: HashMap<String, HashMap<String, String>>,
    /// Claude turns queued by the operator while another Claude turn is still running.
    /// Runtime-only.
    #[serde(skip)]
    pub queued_claude_turns: VecDeque<QueuedClaudeTurn>,
    /// Gemini models discovered from the backend (CLI/API). Runtime-only.
    #[serde(skip)]
    pub gemini_models: Vec<String>,
    /// Gemini model discovery error (if any). Runtime-only.
    #[serde(skip)]
    pub gemini_models_error: Option<String>,
    /// In-flight intake-agent decision deferred until `TurnCompleted` /
    /// `TurnFailed` lands for the synthetic intake lane. Runtime-only.
    #[serde(skip)]
    pub pending_intake: Option<PendingIntake>,
    /// Operator-overridable lane id used for intake turns. When `None`,
    /// `intake::start` clones the dispatching agent's lane — but only
    /// when that lane is claude-class, since the intake system prompt
    /// and 30s timeout are calibrated for haiku-style classifiers.
    /// Setting this to a claude lane id lets a future operator setup
    /// run a cheap claude preprocessor in front of a non-claude writer.
    /// Runtime-only.
    #[serde(skip)]
    pub intake_agent_id: Option<String>,
}

impl AgentsState {
    pub fn selected_context_agent(&self) -> Option<&str> {
        self.selected_agent.as_deref().or_else(|| {
            self.agents
                .get(self.roster_selected)
                .map(|agent| agent.id.as_str())
        })
    }

    pub fn selected_context_mission(&self) -> Option<&str> {
        self.selected_mission.as_deref().or_else(|| {
            self.missions
                .get(self.mission_selected)
                .map(|mission| mission.id.as_str())
        })
    }

    pub fn note_event(&mut self) {
        self.event_epoch = self.event_epoch.wrapping_add(1);
    }

    /// Look an agent up by id in O(1) via `agents_index`. Falls back to
    /// a linear scan if the index is stale (e.g. test code mutated the
    /// vec without going through the helpers); the next mutating event
    /// that calls `rebuild_agents_index` repairs it.
    pub fn agents_get(&self, id: &str) -> Option<&AgentLane> {
        if let Some(&idx) = self.agents_index.get(id) {
            if let Some(lane) = self.agents.get(idx) {
                if lane.id == id {
                    return Some(lane);
                }
            }
        }
        self.agents.iter().find(|a| a.id == id)
    }

    pub fn agents_get_mut(&mut self, id: &str) -> Option<&mut AgentLane> {
        if let Some(&idx) = self.agents_index.get(id) {
            if matches!(self.agents.get(idx), Some(lane) if lane.id == id) {
                return self.agents.get_mut(idx);
            }
        }
        self.agents.iter_mut().find(|a| a.id == id)
    }

    /// Rebuild `agents_index` from the current `agents` vec. Cheap; call
    /// after any code path that pushes / removes / reorders agents
    /// outside of `agents_push` / `agents_remove_id`.
    pub fn rebuild_agents_index(&mut self) {
        self.agents_index.clear();
        self.agents_index.reserve(self.agents.len());
        for (idx, lane) in self.agents.iter().enumerate() {
            self.agents_index.insert(lane.id.clone(), idx);
        }
    }
}

impl Default for AgentsState {
    fn default() -> Self {
        Self {
            selected_agent: None,
            selected_mission: None,
            dock_tab: AgentOpsTab::Roster,
            console_tab: AgentConsoleTab::Thread,
            chat_input: String::new(),
            chat_input_cursor: 0,
            chat_input_selection_anchor: None,
            chat_input_scroll: chat_input_scroll_default(),
            chat_prompt_history: Vec::new(),
            chat_prompt_history_pos: None,
            chat_prompt_history_draft: None,
            chat_channel: AgentChannel::Agent,
            swarm_default_template: swarm_default_template_default(),
            swarm_default_mission: swarm_default_mission_default(),
            swarm_role_by_agent_id: HashMap::new(),
            swarm_priority_agent_ids: HashSet::new(),
            codex_max_parallel_turns: codex_max_parallel_turns_default(),
            claude_max_parallel_turns: claude_max_parallel_turns_default(),
            agents: Vec::new(),
            agents_index: HashMap::new(),
            missions: Vec::new(),
            patches: Vec::new(),
            messages: Vec::new(),
            evidence: Vec::new(),
            alerts: Vec::new(),
            diag_events: Vec::new(),
            mcp: default_offline_mcp_status(),
            roster_selected: 0,
            roster_selected_backend: None,
            roster_tree_selected: None,
            roster_expanded_backend_kinds: HashSet::new(),
            roster_tree_collapsed_agent_ids: HashSet::new(),
            mission_selected: 0,
            alert_selected: 0,
            patch_selected: 0,
            artifacts_selected: 0,
            artifacts_collapsed_prompts: HashSet::new(),
            artifacts_popup_open: false,
            artifacts_popup_scroll: 0,
            artifacts_popup_last_max_scroll: usize::MAX,
            artifacts_popup_chat_input: String::new(),
            artifacts_popup_chat_cursor: 0,
            artifacts_popup_chat_selection_anchor: None,
            artifacts_popup_chat_scroll: chat_input_scroll_default(),
            artifacts_history_popup_open: false,
            artifacts_history_popup_scroll: 0,
            artifacts_history_selected: 0,
            artifacts_selected_saved_run_path: None,
            artifacts_history_filter: SavedRunHistoryFilter::All,
            artifacts_history_pending_action: None,
            global_archive_open: false,
            global_archive_query: String::new(),
            global_archive_query_cursor: 0,
            global_archive_selected: 0,
            global_archive_scroll: 0,
            global_archive_filter: SavedRunHistoryFilter::All,
            global_archive_index: Vec::new(),
            global_archive_filtered: Vec::new(),
            global_archive_opened_entry: None,
            ops_scroll: 0,
            ops_viewport_width: 0,
            ops_viewport_height: 0,
            console_scroll: CONSOLE_SCROLL_BOTTOM,
            console_max_scroll: 0,
            console_rows_cache: AgentConsoleRowsCache::default(),
            event_epoch: 0,
            pending_provenance_mission_ids: Vec::new(),
            pending_provenance_agent_ids: Vec::new(),
            pending_legacy_notes_alert: None,
            codex_effective_context_window_tokens: HashMap::new(),
            codex_estimated_tokens_used_by_mission: HashMap::new(),
            codex_default_reasoning_effort: HashMap::new(),
            codex_supported_reasoning_efforts: HashMap::new(),
            codex_selected_reasoning_effort: HashMap::new(),
            codex_context_remaining_pct: HashMap::new(),
            codex_mission_context_remaining_pct: HashMap::new(),
            codex_used_tokens: HashMap::new(),
            codex_mission_used_tokens: HashMap::new(),
            codex_turn_prompt_idx: HashMap::new(),
            codex_thread_ids: HashMap::new(),
            codex_mission_thread_ids: HashMap::new(),
            active_turns: HashMap::new(),
            queued_codex_turns: VecDeque::new(),
            workspace_scan_progress: None,
            codex_cli_available: false,
            claude_cli_available: false,
            gemini_cli_available: false,
            claude_models: Vec::new(),
            claude_models_error: None,
            claude_effective_context_window_tokens: HashMap::new(),
            claude_estimated_tokens_used_by_mission: HashMap::new(),
            claude_default_effort: HashMap::new(),
            claude_supported_efforts: HashMap::new(),
            claude_selected_effort: HashMap::new(),
            claude_context_remaining_pct: HashMap::new(),
            claude_mission_context_remaining_pct: HashMap::new(),
            claude_used_tokens: HashMap::new(),
            claude_mission_used_tokens: HashMap::new(),
            claude_turn_prompt_idx: HashMap::new(),
            claude_session_ids: HashMap::new(),
            claude_mission_session_ids: HashMap::new(),
            queued_claude_turns: VecDeque::new(),
            gemini_models: Vec::new(),
            gemini_models_error: None,
            pending_intake: None,
            intake_agent_id: None,
        }
    }
}
