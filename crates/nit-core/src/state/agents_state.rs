use std::collections::HashMap;

use super::*;

const OFFLINE_MCP_ENDPOINT: &str = "mock://offline";

/// Initial MCP status before any connection attempt — the CLI overrides
/// the placeholder endpoint as soon as it discovers the real bus address.
fn default_offline_mcp_status() -> McpStatus {
    McpStatus {
        state: McpConnectionState::Disconnected,
        endpoint: OFFLINE_MCP_ENDPOINT.into(),
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
    /// Char index in `chat_input` where a selection started.
    #[serde(skip)]
    pub chat_input_selection_anchor: Option<usize>,
    #[serde(skip, default = "chat_input_scroll_default")]
    pub chat_input_scroll: usize,
    /// Previously submitted prompts. `#[serde(skip)]` because operator
    /// prompts must not be persisted.
    #[serde(skip)]
    pub chat_prompt_history: Vec<String>,
    /// Index into `chat_prompt_history` while cycling via Up/Down.
    #[serde(skip)]
    pub chat_prompt_history_pos: Option<usize>,
    /// Draft compose text captured when entering history navigation.
    #[serde(skip)]
    pub chat_prompt_history_draft: Option<String>,
    pub chat_channel: AgentChannel,
    /// Default swarm template, applied when `@swarm` omits `template=...`
    /// or when prompt auto-detection fires.
    #[serde(default = "swarm_default_template_default")]
    pub swarm_default_template: String,
    /// Default swarm mission preset. `auto` preserves prompt-based
    /// detection; other values pin the mission kind.
    #[serde(default = "swarm_default_mission_default")]
    pub swarm_default_mission: String,
    /// Per-agent role hint for `parallel` / `bulk` planning. Missing
    /// entry means "all roles".
    #[serde(default)]
    pub swarm_role_by_agent_id: HashMap<String, String>,
    /// Agents marked priority in the roster (planning hint).
    #[serde(default)]
    pub swarm_priority_agent_ids: HashSet<String>,
    /// Concurrency caps from `--codex-max-parallel-turns` and the Claude
    /// equivalent. Used as auto-start swarm-size hints.
    #[serde(skip, default = "codex_max_parallel_turns_default")]
    pub codex_max_parallel_turns: usize,
    #[serde(skip, default = "claude_max_parallel_turns_default")]
    pub claude_max_parallel_turns: usize,
    pub agents: Vec<AgentLane>,
    /// `agents[i].id -> i` cache for O(1) lookups. Production write paths
    /// (`agent_bus::upsert_agent`, `swarm::clones`, `multipane::setup`)
    /// keep it in sync; `agents_get` / `agents_get_mut` linear-scan-fall-back
    /// when the entry is stale so direct `agents.push` from tests remains
    /// correct. `rebuild_agents_index` repairs after bulk mutations.
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
    /// Selected backend group row when focus is on a backend rather than
    /// a concrete model row.
    #[serde(skip)]
    pub roster_selected_backend: Option<AgentLaneKind>,
    /// Selected leaf inside the focused agent's Size/Role tree, used by
    /// keyboard navigation.
    #[serde(skip)]
    pub roster_tree_selected: Option<RosterTreeSelection>,
    /// Backends whose model rows are expanded in the roster.
    #[serde(skip, default)]
    pub roster_expanded_backend_kinds: HashSet<AgentLaneKind>,
    /// Agents whose Size/Role subtree is collapsed.
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
    /// Cached `max_scroll` from the last render so wheel/keyboard handlers
    /// can clamp without rebuilding the rendered markdown each input event.
    /// `usize::MAX` is the "no render yet, scroll unclamped" sentinel.
    #[serde(skip, default = "artifacts_popup_last_max_scroll_default")]
    pub artifacts_popup_last_max_scroll: usize,
    #[serde(skip)]
    pub artifacts_popup_chat_input: String,
    #[serde(skip)]
    pub artifacts_popup_chat_cursor: usize,
    #[serde(skip)]
    pub artifacts_popup_chat_selection_anchor: Option<usize>,
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
    /// Full index built when the popup opens.
    #[serde(skip)]
    pub global_archive_index: Vec<GlobalArchiveEntry>,
    /// Filtered results as `(score, index_into_global_archive_index)`.
    #[serde(skip)]
    pub global_archive_filtered: Vec<(i64, usize)>,
    /// Entry under inspection — the artifact popup loads content directly
    /// from this entry's `run.json` rather than re-scanning by card index.
    #[serde(skip)]
    pub global_archive_opened_entry: Option<GlobalArchiveEntry>,
    #[serde(skip)]
    pub ops_scroll: usize,
    #[serde(skip)]
    pub ops_viewport_width: usize,
    #[serde(skip)]
    pub ops_viewport_height: usize,
    /// `CONSOLE_SCROLL_BOTTOM` (`usize::MAX`) means "auto-scroll to bottom".
    #[serde(skip)]
    pub console_scroll: usize,
    /// Cached `max_scroll` so input handlers can clamp without rebuilding
    /// the console.
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
    // --- Codex model metadata, all keyed by model slug ---
    // Populated when the roster is seeded from `~/.codex/models_cache.json`.
    #[serde(skip)]
    pub codex_effective_context_window_tokens: HashMap<String, u32>,
    /// Heuristic estimate of context tokens used, per mission thread.
    #[serde(skip)]
    pub codex_estimated_tokens_used_by_mission: HashMap<String, u32>,
    /// Default reasoning effort per model (`low`/`medium`/`high`/`xhigh`).
    #[serde(skip)]
    pub codex_default_reasoning_effort: HashMap<String, String>,
    /// Reasoning effort sizes the model accepts.
    #[serde(skip)]
    pub codex_supported_reasoning_efforts: HashMap<String, Vec<String>>,
    /// Operator-selected reasoning effort. Defaults to
    /// `codex_default_reasoning_effort`; mutated from the roster UI.
    #[serde(skip)]
    pub codex_selected_reasoning_effort: HashMap<String, String>,
    /// Context-remaining percentage for the currently dispatched Codex turn.
    #[serde(skip)]
    pub codex_context_remaining_pct: HashMap<String, u8>,
    /// Last-known context-remaining percentage per mission thread,
    /// keyed first by mission id then by model slug.
    #[serde(skip)]
    pub codex_mission_context_remaining_pct: HashMap<String, HashMap<String, u8>>,
    /// Total tokens used in non-mission ad-hoc chat.
    #[serde(skip)]
    pub codex_used_tokens: HashMap<String, u32>,
    /// Total tokens used per mission thread.
    #[serde(skip)]
    pub codex_mission_used_tokens: HashMap<String, HashMap<String, u32>>,
    /// `agent_id → user-prompt index` linking dispatched turns to their
    /// triggering message; consumed by `TurnCompleted` to wire the reply.
    #[serde(skip)]
    pub codex_turn_prompt_idx: HashMap<String, usize>,
    /// Per-model Codex thread ids for ad-hoc (non-mission) chat resumption.
    #[serde(skip)]
    pub codex_thread_ids: HashMap<String, String>,
    /// Per-mission Codex thread ids for live-mission resumption.
    #[serde(skip)]
    pub codex_mission_thread_ids: HashMap<String, HashMap<String, String>>,
    /// Active backend turn telemetry keyed by agent id.
    #[serde(skip)]
    pub active_turns: HashMap<String, AgentTurnState>,
    /// Codex turns queued behind an in-flight Codex turn.
    #[serde(skip)]
    pub queued_codex_turns: VecDeque<QueuedCodexTurn>,
    /// `Some((done, total))` while the workspace-wide genome scan is in
    /// flight, `None` when idle. Drives the "Evaluating genome: X/Y files"
    /// breather in the agent console.
    #[serde(skip)]
    pub workspace_scan_progress: Option<(usize, usize)>,
    // --- CLI availability (read at startup for the roster's backend inventory) ---
    #[serde(skip)]
    pub codex_cli_available: bool,
    #[serde(skip)]
    pub claude_cli_available: bool,
    #[serde(skip)]
    pub gemini_cli_available: bool,
    // --- Claude model metadata, structurally parallel to the Codex block ---
    #[serde(skip)]
    pub claude_models: Vec<String>,
    #[serde(skip)]
    pub claude_models_error: Option<String>,
    #[serde(skip)]
    pub claude_effective_context_window_tokens: HashMap<String, u32>,
    #[serde(skip)]
    pub claude_estimated_tokens_used_by_mission: HashMap<String, u32>,
    /// Default effort level (`low`/`medium`/`high`/`max`).
    #[serde(skip)]
    pub claude_default_effort: HashMap<String, String>,
    #[serde(skip)]
    pub claude_supported_efforts: HashMap<String, Vec<String>>,
    /// Operator-selected effort. Defaults to `claude_default_effort`.
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
    /// `agent_id → user-prompt index` for the active Claude turn. Mirrors
    /// `codex_turn_prompt_idx`'s role for the Codex side.
    #[serde(skip)]
    pub claude_turn_prompt_idx: HashMap<String, usize>,
    /// Per-model Claude session ids for ad-hoc resumption.
    #[serde(skip)]
    pub claude_session_ids: HashMap<String, String>,
    /// Per-mission Claude session ids for live-mission resumption.
    #[serde(skip)]
    pub claude_mission_session_ids: HashMap<String, HashMap<String, String>>,
    /// Claude turns queued behind an in-flight Claude turn.
    #[serde(skip)]
    pub queued_claude_turns: VecDeque<QueuedClaudeTurn>,
    #[serde(skip)]
    pub gemini_models: Vec<String>,
    #[serde(skip)]
    pub gemini_models_error: Option<String>,
    /// Intake decision deferred until `TurnCompleted` / `TurnFailed` lands
    /// for the synthetic intake lane.
    #[serde(skip)]
    pub pending_intake: Option<PendingIntake>,
    /// Operator override of the intake lane. `None` ⇒ `intake::start`
    /// clones the dispatching agent's lane — but only when that lane is
    /// claude-class, because the intake system prompt and 30 s timeout
    /// are tuned for haiku-style classifiers. Setting this to a claude
    /// lane id lets a future setup pre-classify with cheap claude even
    /// when the writer is a non-claude backend.
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

    /// O(1) lookup via `agents_index`, falling back to a linear scan when
    /// the cached index is stale. Test code that mutates `agents` directly
    /// is the usual culprit; the next event-handler call to
    /// `rebuild_agents_index` repairs the cache.
    pub fn agents_get(&self, id: &str) -> Option<&AgentLane> {
        if let Some(idx) = self.cached_agent_index(id) {
            return self.agents.get(idx);
        }
        self.agents.iter().find(|a| a.id == id)
    }

    pub fn agents_get_mut(&mut self, id: &str) -> Option<&mut AgentLane> {
        if let Some(idx) = self.cached_agent_index(id) {
            return self.agents.get_mut(idx);
        }
        self.agents.iter_mut().find(|a| a.id == id)
    }

    fn cached_agent_index(&self, id: &str) -> Option<usize> {
        let &idx = self.agents_index.get(id)?;
        self.agents
            .get(idx)
            .filter(|lane| lane.id == id)
            .map(|_| idx)
    }

    /// Repopulate `agents_index` after a bulk mutation that bypassed
    /// `agents_push` / `agents_remove_id`. Cheap — O(n) scan over `agents`.
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
