use crate::{
    actions::Action,
    buffer::Buffer,
    config::{GolSeedSource, Settings},
    gol_rules::{RuleCatalog, SelectedRule},
    io,
    lab::AppKind,
    mode::Mode,
    pane::PaneId,
    prompt::Prompt,
    rule_protocol::{RuleMode, RuleRef},
    search::{FuzzySearchState, SearchMode},
    seed::{SeedEncoderId, SeedParams, SeedPreviewMode, SeedStats, SeedViewMode},
    viewport::Viewport,
};
use nit_games::analysis::AnalysisConfig;
use nit_gol::Rule;
use nit_gol::{AttractorEvent, AutoStopPolicy};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_LOG_CAPACITY: usize = 512;

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GamesStatus {
    Idle,
    Running,
    Paused,
    Done,
    Error,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GamesAnalysisRequest {
    pub path: Option<String>,
    pub tail_rounds: usize,
    pub trajectory_samples: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GamesReplayRequest {
    pub a_id: String,
    pub b_id: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GamesAnalysisState {
    pub open: bool,
    pub running: bool,
    pub source_path: Option<String>,
    pub last_error: Option<String>,
    #[serde(skip)]
    pub summary: Option<nit_games::analysis::HistoryAnalysisSummary>,
    #[serde(skip)]
    pub preview: Option<nit_games::analysis::HistoryAnalysisPreview>,
    #[serde(skip)]
    pub scroll_offset: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GamesRunEntry {
    pub label: String,
    pub summary_path: String,
    pub run_dir: Option<String>,
    pub seed: Option<u64>,
    pub timestamp: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GamesRunBrowserState {
    pub open: bool,
    pub loading: bool,
    pub last_error: Option<String>,
    #[serde(skip)]
    pub entries: Vec<GamesRunEntry>,
    #[serde(skip)]
    pub selected: usize,
    #[serde(skip)]
    pub scroll_offset: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GamesReplayState {
    pub open: bool,
    pub loading: bool,
    pub last_error: Option<String>,
    pub selected_pair: Option<(String, String)>,
    #[serde(skip)]
    pub pairs: Vec<(String, String)>,
    #[serde(skip)]
    pub title: Option<String>,
    #[serde(skip)]
    pub lines: Vec<String>,
    #[serde(skip)]
    pub scroll_offset: usize,
    #[serde(skip)]
    pub selected_index: usize,
    #[serde(skip)]
    pub cycle: Option<nit_games::CycleMetadata>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GamesStrategyInspectState {
    pub open: bool,
    pub last_error: Option<String>,
    #[serde(skip)]
    pub title: Option<String>,
    #[serde(skip)]
    pub lines: Vec<String>,
    #[serde(skip)]
    pub definition: Option<nit_games::output::StrategyDefinition>,
    #[serde(skip)]
    pub selected_index: usize,
    #[serde(skip)]
    pub scroll_offset: usize,
    #[serde(skip)]
    pub definitions: Vec<nit_games::output::StrategyDefinition>,
    #[serde(skip)]
    pub source_label: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GamesTmSimState {
    pub open: bool,
    pub last_error: Option<String>,
    #[serde(skip)]
    pub definition: Option<nit_games::output::StrategyDefinition>,
    #[serde(skip)]
    pub input: Option<u64>,
    #[serde(skip)]
    pub steps_override: Option<u32>,
    #[serde(skip)]
    pub source_label: Option<String>,
    #[serde(skip)]
    pub scroll_offset: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GamesCaSimState {
    pub open: bool,
    pub last_error: Option<String>,
    #[serde(skip)]
    pub definition: Option<nit_games::output::StrategyDefinition>,
    #[serde(skip)]
    pub input: Option<u64>,
    #[serde(skip)]
    pub steps_override: Option<u32>,
    #[serde(skip)]
    pub source_label: Option<String>,
    #[serde(skip)]
    pub scroll_offset: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GamesMatchHistoryState {
    pub open: bool,
    pub last_error: Option<String>,
    #[serde(skip)]
    pub capture_disabled_for_run: bool,
    #[serde(skip)]
    pub entries: Vec<nit_games::MatchHistoryPreview>,
    #[serde(skip)]
    pub total_entries: usize,
    #[serde(skip)]
    pub loaded_start: usize,
    #[serde(skip)]
    pub max_rounds_seen: usize,
    #[serde(skip)]
    pub column_offset: usize,
    #[serde(skip)]
    pub round_limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct GamesRunOverride {
    pub config: nit_games::NormalizedConfig,
    pub config_text: String,
    pub label: String,
    pub family_mode: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FamilyRunBuildTimings {
    pub generated_strategies: usize,
    pub generation_elapsed: Duration,
    pub estimate_elapsed: Duration,
    pub normalize_elapsed: Duration,
    pub tm_filter_elapsed: Option<Duration>,
    pub tm_filter: Option<nit_games::TmHaltingFilterDiagnostics>,
    pub total_elapsed: Duration,
}

#[derive(Clone, Debug)]
pub struct GamesFamilyRunRequest {
    pub family: String,
    pub input: String,
    pub force: bool,
}

#[derive(Clone, Debug)]
pub struct GamesConfigPreview {
    pub version: u64,
    pub result: Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UiSelectionPane {
    JobOutput,
    AgentConsole,
    VisualizerMain,
    VisualizerSide,
    GateMonitor,
    GamesPetriDish,
    HelpPopup,
    ArtifactsPopup,
    ArtifactsHistoryPopup,
    GamesAnalysisPopup,
    GamesRunBrowserPopup,
    GamesReplayPopup,
    GamesStrategyPopup,
    GamesTmSimPopupLeft,
    GamesTmSimPopupRight,
    GamesCaSimPopupLeft,
    GamesCaSimPopupRight,
    GamesMatchHistoryPopup,
}

#[derive(Copy, Clone, Debug)]
pub struct UiSelection {
    pub pane: UiSelectionPane,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentOpsTab {
    Roster,
    Missions,
    Dag,
    Mcp,
    Alerts,
    Patch,
    Evidence,
    Diagnostics,
    Scratchpad,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SavedRunHistoryFilter {
    #[default]
    All,
    LastDay,
    LastWeek,
    LastMonth,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SavedRunHistoryPendingAction {
    DeleteSelected,
    PruneFiltered,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalArchiveSourceKind {
    Mission,
    AdHoc,
}

#[derive(Clone, Debug)]
pub struct GlobalArchiveEntry {
    /// "PROMPT" | "REPLY" | "PATCH" | "EVIDENCE"
    pub kind: &'static str,
    pub owner: String,
    /// Truncated display preview (~120 chars).
    pub preview: String,
    /// Mission title or "ad-hoc: {agent_id}".
    pub source: String,
    /// The mission_id or agent_id.
    pub source_id: String,
    pub source_kind: GlobalArchiveSourceKind,
    /// Relative label ("saved 2h ago").
    pub time_label: String,
    pub archive_micros: Option<u128>,
    /// Path to the run.json containing this artifact.
    pub run_path: String,
    /// Index of this artifact within the run's messages/patches/evidence arrays.
    pub artifact_index: usize,
    /// Pre-lowercased haystack for fuzzy matching: kind + owner + source + full_text.
    pub search_hay: String,
    /// Lowercased word tokens extracted from the full content (for BM25 scoring).
    pub search_tokens: Vec<String>,
}

impl AgentOpsTab {
    pub fn label(self) -> &'static str {
        match self {
            AgentOpsTab::Roster => "ROSTER",
            AgentOpsTab::Missions => "MISSIONS",
            AgentOpsTab::Dag => "DAG",
            AgentOpsTab::Mcp => "MCP",
            AgentOpsTab::Alerts => "ALERTS",
            AgentOpsTab::Patch => "PATCH",
            AgentOpsTab::Evidence => "ARTIFACTS",
            AgentOpsTab::Diagnostics => "DIAG",
            AgentOpsTab::Scratchpad => "SCRATCHPAD",
        }
    }

    pub fn next(self) -> Self {
        match self {
            AgentOpsTab::Roster => AgentOpsTab::Missions,
            AgentOpsTab::Missions => AgentOpsTab::Dag,
            AgentOpsTab::Dag => AgentOpsTab::Evidence,
            AgentOpsTab::Evidence => AgentOpsTab::Mcp,
            AgentOpsTab::Mcp => AgentOpsTab::Alerts,
            AgentOpsTab::Alerts => AgentOpsTab::Diagnostics,
            // Legacy/hidden patch tab: skip forward into Artifacts.
            AgentOpsTab::Patch => AgentOpsTab::Evidence,
            AgentOpsTab::Diagnostics => AgentOpsTab::Scratchpad,
            AgentOpsTab::Scratchpad => AgentOpsTab::Roster,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            AgentOpsTab::Roster => AgentOpsTab::Scratchpad,
            AgentOpsTab::Missions => AgentOpsTab::Roster,
            AgentOpsTab::Dag => AgentOpsTab::Missions,
            AgentOpsTab::Evidence => AgentOpsTab::Dag,
            AgentOpsTab::Mcp => AgentOpsTab::Evidence,
            AgentOpsTab::Alerts => AgentOpsTab::Mcp,
            // Legacy/hidden patch tab: skip backward into Alerts.
            AgentOpsTab::Patch => AgentOpsTab::Alerts,
            AgentOpsTab::Diagnostics => AgentOpsTab::Alerts,
            AgentOpsTab::Scratchpad => AgentOpsTab::Diagnostics,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentConsoleTab {
    Thread,
    Patch,
    Evidence,
    Diagnostics,
    Scratchpad,
}

impl AgentConsoleTab {
    pub fn label(self) -> &'static str {
        match self {
            AgentConsoleTab::Thread => "THREAD",
            AgentConsoleTab::Patch => "PATCH",
            AgentConsoleTab::Evidence => "ARTIFACTS",
            AgentConsoleTab::Diagnostics => "DIAG",
            AgentConsoleTab::Scratchpad => "SCRATCHPAD",
        }
    }

    pub fn next(self) -> Self {
        match self {
            AgentConsoleTab::Thread => AgentConsoleTab::Patch,
            AgentConsoleTab::Patch => AgentConsoleTab::Evidence,
            AgentConsoleTab::Evidence => AgentConsoleTab::Diagnostics,
            AgentConsoleTab::Diagnostics => AgentConsoleTab::Scratchpad,
            AgentConsoleTab::Scratchpad => AgentConsoleTab::Thread,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            AgentConsoleTab::Thread => AgentConsoleTab::Scratchpad,
            AgentConsoleTab::Patch => AgentConsoleTab::Thread,
            AgentConsoleTab::Evidence => AgentConsoleTab::Patch,
            AgentConsoleTab::Diagnostics => AgentConsoleTab::Evidence,
            AgentConsoleTab::Scratchpad => AgentConsoleTab::Diagnostics,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentStatus {
    Idle,
    Running,
    Waiting,
    Error,
}

impl AgentStatus {
    pub fn label(self) -> &'static str {
        match self {
            AgentStatus::Idle => "IDLE",
            AgentStatus::Running => "RUNNING",
            AgentStatus::Waiting => "WAITING",
            AgentStatus::Error => "ERROR",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MissionPhase {
    Plan,
    Execute,
    Verify,
    Report,
}

impl MissionPhase {
    pub fn label(self) -> &'static str {
        match self {
            MissionPhase::Plan => "PLAN",
            MissionPhase::Execute => "EXECUTE",
            MissionPhase::Verify => "VERIFY",
            MissionPhase::Report => "REPORT",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum McpConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

impl McpConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            McpConnectionState::Disconnected => "DISCONNECTED",
            McpConnectionState::Connecting => "CONNECTING",
            McpConnectionState::Connected => "CONNECTED",
            McpConnectionState::Error => "ERROR",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PatchStatus {
    New,
    Reviewed,
    Applied,
    Rejected,
}

impl PatchStatus {
    pub fn label(self) -> &'static str {
        match self {
            PatchStatus::New => "NEW",
            PatchStatus::Reviewed => "REVIEWED",
            PatchStatus::Applied => "APPLIED",
            PatchStatus::Rejected => "REJECTED",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentAlertSeverity {
    Info,
    Warn,
    Error,
}

impl AgentAlertSeverity {
    pub fn label(self) -> &'static str {
        match self {
            AgentAlertSeverity::Info => "INFO",
            AgentAlertSeverity::Warn => "WARN",
            AgentAlertSeverity::Error => "ERROR",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentChannel {
    Agent,
    Broadcast,
}

#[derive(
    Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentLaneKind {
    #[default]
    Unknown,
    Mock,
    Codex,
    Claude,
    Gemini,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentLane {
    pub id: String,
    pub role: String,
    pub lane: String,
    #[serde(default)]
    pub kind: AgentLaneKind,
    pub status: AgentStatus,
    pub heartbeat_age_secs: u64,
    pub queue_len: usize,
    pub current_mission: Option<String>,
    pub last_message: String,
}

impl AgentLane {
    pub fn is_codex(&self) -> bool {
        matches!(self.kind, AgentLaneKind::Codex)
            || (matches!(self.kind, AgentLaneKind::Unknown)
                && self.lane.eq_ignore_ascii_case("codex"))
    }

    pub fn is_claude(&self) -> bool {
        matches!(self.kind, AgentLaneKind::Claude)
            || (matches!(self.kind, AgentLaneKind::Unknown)
                && self.lane.eq_ignore_ascii_case("claude"))
    }

    pub fn supports_swarm_priority(&self) -> bool {
        let backend_supports_priority = matches!(
            self.kind,
            AgentLaneKind::Codex | AgentLaneKind::Claude | AgentLaneKind::Gemini
        ) || (matches!(self.kind, AgentLaneKind::Unknown)
            && (self.lane.eq_ignore_ascii_case("codex")
                || self.lane.eq_ignore_ascii_case("claude")
                || self.lane.eq_ignore_ascii_case("gemini")));
        backend_supports_priority && !self.role.eq_ignore_ascii_case(&self.lane)
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MissionRecord {
    pub id: String,
    pub title: String,
    pub phase: MissionPhase,
    pub swarm: bool,
    pub assigned_agents: Vec<String>,
    pub status: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct McpStatus {
    pub state: McpConnectionState,
    pub endpoint: String,
    pub latency_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentAlert {
    pub severity: AgentAlertSeverity,
    pub source: String,
    pub message: String,
    pub at: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentMessage {
    pub at: String,
    pub channel: AgentChannel,
    pub agent_id: Option<String>,
    pub mission_id: Option<String>,
    pub text: String,
    /// Index of the user prompt message that this reply is responding to.
    /// `None` for user prompts themselves, or for replies where the prompt is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_msg_idx: Option<usize>,
    /// Optional kind tag for special message types (e.g. "synth" for synthesis reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PatchProposal {
    pub id: String,
    pub mission_id: Option<String>,
    pub agent_id: String,
    pub title: String,
    pub summary: String,
    pub diff: String,
    pub status: PatchStatus,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EvidenceItem {
    pub id: String,
    pub mission_id: Option<String>,
    pub agent_id: Option<String>,
    pub title: String,
    pub detail: String,
    pub link: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentDiagnosticEvent {
    pub severity: AgentAlertSeverity,
    pub source: String,
    pub message: String,
    pub at: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AgentConsoleRowKind {
    User,
    Agent,
    ArtifactLink,
    Breather,
    StatusHeader,
    StatusRow,
    StatusSubRow,
}

#[derive(Clone, Debug)]
pub struct AgentConsoleRow {
    pub text: String,
    pub kind: AgentConsoleRowKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConsoleRowsCacheKey {
    pub width: usize,
    pub mission: Option<String>,
    pub agent: Option<String>,
    pub messages_len: usize,
    pub event_epoch: u64,
}

#[derive(Clone, Debug, Default)]
pub struct AgentConsoleRowsCache {
    pub key: Option<AgentConsoleRowsCacheKey>,
    pub rows: Vec<AgentConsoleRow>,
    pub last_message_was_user: bool,
    /// `(row_index, prompt_msg_idx)` — positions inside `rows` where an inline
    /// breather should be spliced in when agents are still pending for that prompt.
    pub breather_slots: Vec<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct AgentTurnState {
    pub started_at: Instant,
    pub last_heartbeat_at: Instant,
    pub last_output_at: Instant,
    pub stage: Option<String>,
}

/// Real-time shadow evaluation result for a single file during an agent turn.
#[derive(Clone, Debug)]
pub struct GenomeShadowEval {
    pub tier: crate::genome_report::GenomeTier,
    pub quality: &'static str,
    pub consistency: f32,
    /// "improved", "degraded", "unchanged", or "new".
    pub delta_label: &'static str,
    pub is_new_file: bool,
    pub at: Instant,
}

#[derive(Clone, Debug)]
pub struct QueuedCodexTurn {
    pub agent_id: String,
    pub mission_id: Option<String>,
    pub prompt: String,
    /// Index of the user prompt message that triggered this queued turn.
    pub prompt_msg_idx: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct QueuedClaudeTurn {
    pub agent_id: String,
    pub mission_id: Option<String>,
    pub prompt: String,
    /// Index of the user prompt message that triggered this queued turn.
    pub prompt_msg_idx: Option<usize>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RosterTreeBranch {
    Size,
    Role,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RosterTreeSelection {
    pub branch: RosterTreeBranch,
    pub leaf_idx: usize,
}

/// Sentinel value for `AgentsState::console_scroll` meaning "auto-scroll to bottom".
pub const CONSOLE_SCROLL_BOTTOM: usize = usize::MAX;

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
}

fn chat_input_scroll_default() -> usize {
    usize::MAX
}

fn swarm_default_template_default() -> String {
    "lab".into()
}

fn swarm_default_mission_default() -> String {
    "auto".into()
}

fn codex_max_parallel_turns_default() -> usize {
    2
}

fn claude_max_parallel_turns_default() -> usize {
    2
}

impl AgentsState {
    pub fn default_with_mocks() -> Self {
        let agents = vec![
            AgentLane {
                id: "planner".into(),
                role: "Planner".into(),
                lane: "Lane A".into(),
                kind: AgentLaneKind::Mock,
                status: AgentStatus::Running,
                heartbeat_age_secs: 1,
                queue_len: 1,
                current_mission: Some("mis-001".into()),
                last_message: "Drafted execution plan".into(),
            },
            AgentLane {
                id: "coder".into(),
                role: "Coder".into(),
                lane: "Lane B".into(),
                kind: AgentLaneKind::Mock,
                status: AgentStatus::Running,
                heartbeat_age_secs: 2,
                queue_len: 2,
                current_mission: Some("mis-001".into()),
                last_message: "Generated 2 patch proposals".into(),
            },
            AgentLane {
                id: "reviewer".into(),
                role: "Reviewer".into(),
                lane: "Lane C".into(),
                kind: AgentLaneKind::Mock,
                status: AgentStatus::Waiting,
                heartbeat_age_secs: 5,
                queue_len: 0,
                current_mission: Some("mis-001".into()),
                last_message: "Waiting for patch review".into(),
            },
        ];
        let missions = vec![MissionRecord {
            id: "mis-001".into(),
            title: "Agent Station MVP refactor".into(),
            phase: MissionPhase::Execute,
            swarm: true,
            assigned_agents: vec!["planner".into(), "coder".into(), "reviewer".into()],
            status: "RUNNING".into(),
            updated_at: "now".into(),
        }];
        let patches = vec![
            PatchProposal {
                id: "patch-001".into(),
                mission_id: Some("mis-001".into()),
                agent_id: "coder".into(),
                title: "Swap pane widgets".into(),
                summary: "Replaces Job Output + Notes render paths with Agent Station widgets."
                    .into(),
                diff: "diff --git a/crates/nit-tui/src/app.rs b/crates/nit-tui/src/app.rs\n@@ -1,3 +1,3 @@\n- job_output_view::render(...)\n- notes_view::render_notes(...)\n+ agent_ops_view::render(...)\n+ agent_console_view::render(...)\n".into(),
                status: PatchStatus::New,
            },
            PatchProposal {
                id: "patch-002".into(),
                mission_id: Some("mis-001".into()),
                agent_id: "reviewer".into(),
                title: "Keybinding alignment".into(),
                summary: "Adds Ctrl+1/2/3 and pane-local tab controls.".into(),
                diff: "diff --git a/crates/nit-tui/src/app.rs b/crates/nit-tui/src/app.rs\n@@ -1,3 +1,7 @@\n+ Ctrl+1 => Editor\n+ Ctrl+2 => Agent Ops\n+ Ctrl+3 => Agent Console\n".into(),
                status: PatchStatus::Reviewed,
            },
        ];
        let messages = Vec::new();
        let evidence = vec![EvidenceItem {
            id: "ev-001".into(),
            mission_id: Some("mis-001".into()),
            agent_id: Some("planner".into()),
            title: "Architecture notes".into(),
            detail: "Reused existing pane IDs to keep layout/config compatibility.".into(),
            link: None,
        }];
        let alerts = vec![AgentAlert {
            severity: AgentAlertSeverity::Info,
            source: "system".into(),
            message: "Agent Station initialized with mock backend.".into(),
            at: "00:00:00".into(),
        }];
        let diag_events = vec![AgentDiagnosticEvent {
            severity: AgentAlertSeverity::Info,
            source: "ops".into(),
            message: "Diagnostics now live under Agent Console.".into(),
            at: "00:00:00".into(),
        }];
        Self {
            selected_agent: agents.first().map(|agent| agent.id.clone()),
            selected_mission: missions.first().map(|mission| mission.id.clone()),
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
            agents,
            missions,
            patches,
            messages,
            evidence,
            alerts,
            diag_events,
            mcp: McpStatus {
                state: McpConnectionState::Connected,
                endpoint: "mock://local-agent-bus".into(),
                latency_ms: Some(6),
                last_error: None,
            },
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
            pending_provenance_mission_ids: vec!["mis-001".into()],
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
        }
    }

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
            missions: Vec::new(),
            patches: Vec::new(),
            messages: Vec::new(),
            evidence: Vec::new(),
            alerts: Vec::new(),
            diag_events: Vec::new(),
            mcp: McpStatus {
                state: McpConnectionState::Disconnected,
                endpoint: "mock://offline".into(),
                latency_ms: None,
                last_error: None,
            },
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
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GamesState {
    pub status: GamesStatus,
    pub running: bool,
    pub paused: bool,
    pub petri_hidden: bool,
    pub steps_per_tick: u32,
    #[serde(skip)]
    pub steps_use_match_units: bool,
    pub last_error: Option<String>,
    #[serde(default)]
    pub runtime: nit_games::RuntimeAcceleratorStats,
    pub last_run: Option<nit_games::output::RunSummary>,
    pub last_run_path: Option<String>,
    pub last_event_path: Option<String>,
    pub last_history_path: Option<String>,
    pub analysis: GamesAnalysisState,
    #[serde(skip)]
    pub petri_lines: Vec<String>,
    #[serde(skip)]
    pub pending_run: bool,
    #[serde(skip)]
    pub pending_run_override: Option<GamesRunOverride>,
    #[serde(skip)]
    pub config_preview: Option<GamesConfigPreview>,
    #[serde(skip)]
    pub config_preview_pending: bool,
    #[serde(skip)]
    pub pending_family_run: Option<GamesFamilyRunRequest>,
    #[serde(skip)]
    pub family_building: bool,
    #[serde(skip)]
    pub pending_close: bool,
    #[serde(skip)]
    pub pending_hide: bool,
    #[serde(skip)]
    pub pending_show: bool,
    #[serde(skip)]
    pub pending_export: bool,
    #[serde(skip)]
    pub pending_analyze: Option<GamesAnalysisRequest>,
    #[serde(skip)]
    pub pending_run_browser: bool,
    #[serde(skip)]
    pub pending_run_load: Option<String>,
    #[serde(skip)]
    pub pending_replay: Option<GamesReplayRequest>,
    #[serde(skip)]
    pub run_browser: GamesRunBrowserState,
    #[serde(skip)]
    pub replay: GamesReplayState,
    #[serde(skip)]
    pub strategy_inspect: GamesStrategyInspectState,
    #[serde(skip)]
    pub tm_sim: GamesTmSimState,
    #[serde(skip)]
    pub ca_sim: GamesCaSimState,
    #[serde(skip)]
    pub match_history: GamesMatchHistoryState,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LogBuffer {
    lines: VecDeque<String>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, line: impl Into<String>) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line.into());
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, String> {
        self.lines.iter()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct JobState {
    pub paused: bool,
    pub progress: f32,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum VisualizerMode {
    SimOnly,
    Search,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum GolRenderMode {
    Solid,
    HalfBlock,
    Braille,
}

impl GolRenderMode {
    pub fn next(self, braille_enabled: bool) -> Self {
        match self {
            GolRenderMode::Solid => GolRenderMode::HalfBlock,
            GolRenderMode::HalfBlock => {
                if braille_enabled {
                    GolRenderMode::Braille
                } else {
                    GolRenderMode::Solid
                }
            }
            GolRenderMode::Braille => GolRenderMode::Solid,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GolRenderMode::Solid => "SOLID",
            GolRenderMode::HalfBlock => "HALF",
            GolRenderMode::Braille => "BRAILLE",
        }
    }

    pub fn effective(self, braille_enabled: bool) -> Self {
        match self {
            GolRenderMode::Braille if !braille_enabled => GolRenderMode::HalfBlock,
            _ => self,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VisualizerRuleEntry {
    pub rule: String,
    pub score: f32,
    pub period: Option<u32>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VisualizerState {
    pub seed: u64,
    pub variant: u8,
    pub mode: VisualizerMode,
    pub seed_encoder: SeedEncoderId,
    pub seed_view: SeedViewMode,
    pub seed_plate_mode: SeedPreviewMode,
    pub seed_params: SeedParams,
    pub seed_stats: SeedStats,
    pub seed_hash: u64,
    pub input_hash: u64,
    pub seed_search_active: bool,
    pub seed_search_rps: u32,
    pub render_mode: GolRenderMode,
    pub running: bool,
    pub age_shading: bool,
    pub trails: bool,
    pub overlay_bbox: bool,
    pub overlay_heat: bool,
    pub scanlines: bool,
    pub paused: bool,
    pub paused_by_attractor: bool,
    pub wrap: bool,
    pub rule: String,
    pub rule_mode: RuleMode,
    pub protocol_name: Option<String>,
    pub generation: u64,
    pub alive: usize,
    pub period: Option<u32>,
    pub auto_stop_policy: AutoStopPolicy,
    pub last_attractor: Option<AttractorEvent>,
    pub tick_ms: u64,
    pub seed_source: GolSeedSource,
    pub search_rps: u32,
    pub leaderboard: Vec<VisualizerRuleEntry>,
    pub last_score: Option<f32>,
    pub seed_show_grid: bool,
    pub seed_show_bbox: bool,
    pub seed_show_halo: bool,
    pub seed_show_components: bool,
    pub seed_show_inset: bool,
    pub seed_scanline: bool,
    pub seed_zoom: u8,
    #[serde(skip)]
    pub inspector_enabled: bool,
    #[serde(skip)]
    pub inspect_ascii_x: usize,
    #[serde(skip)]
    pub inspect_ascii_y: usize,
    #[serde(skip)]
    pub inspect_lifehash_x: usize,
    #[serde(skip)]
    pub inspect_lifehash_y: usize,
    #[serde(skip)]
    pub inspect_hilbert_x: usize,
    #[serde(skip)]
    pub inspect_hilbert_y: usize,
    #[serde(skip)]
    pub inspect_ascii_hash: u64,
    #[serde(skip)]
    pub inspect_lifehash_hash: u64,
    #[serde(skip)]
    pub inspect_hilbert_hash: u64,
    pub seed_snapshots_written: u64,
    pub seed_snapshots_dropped: u64,
    pub seed_snapshot_queue_depth: usize,
    pub seed_last_snapshot_path: Option<String>,
    pub snapshots_written: u64,
    pub snapshots_dropped: u64,
    pub snapshot_queue_depth: usize,
    pub last_snapshot_path: Option<String>,
    #[serde(skip)]
    pub petri_hidden: bool,
    #[serde(skip)]
    pub pending_reseed: bool,
    #[serde(skip)]
    pub pending_apply: bool,
    #[serde(skip)]
    pub pending_snapshot: bool,
    #[serde(skip)]
    pub pending_run: bool,
    #[serde(skip)]
    pub pending_close: bool,
    #[serde(skip)]
    pub pending_hide: bool,
    #[serde(skip)]
    pub pending_show: bool,
    #[serde(skip)]
    pub pending_rule_change: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Metrics {
    pub last_render_ms: u128,
    pub frame_count: u64,
    pub last_action: Option<Action>,
}

#[derive(Clone, Debug, Default)]
pub struct SyntaxDebugInfo {
    pub buffer_version: u64,
    pub snapshot_version: Option<u64>,
    pub engine_state: String,
    pub last_job_ms: Option<u128>,
}

#[derive(Clone, Debug)]
pub struct CommandLine {
    pub input: String,
    pub cursor: usize,
}

impl CommandLine {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
        }
    }

    pub fn insert(&mut self, ch: char) {
        let idx = self.char_idx_to_byte(self.cursor);
        self.input.insert(idx, ch);
        self.cursor = self.cursor.saturating_add(1);
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.char_idx_to_byte(self.cursor);
        let start = self.char_idx_to_byte(self.cursor.saturating_sub(1));
        if start < end {
            self.input.replace_range(start..end, "");
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        let len = self.input.chars().count();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    fn char_idx_to_byte(&self, idx: usize) -> usize {
        if idx == 0 {
            return 0;
        }
        for (count, (byte_idx, _)) in self.input.char_indices().enumerate() {
            if count == idx {
                return byte_idx;
            }
        }
        self.input.len()
    }
}

impl Default for CommandLine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Default)]
pub struct RulePickerState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ProtocolPickerState {
    pub open: bool,
    pub selected: usize,
    pub custom_input: String,
    pub custom_error: Option<String>,
    pub custom_preview: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileTreeKind {
    File,
    Dir,
    Loading,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DirEntryModel {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileTreeRow {
    pub text: String,
    pub path: PathBuf,
    pub kind: FileTreeKind,
    pub depth: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileTreeState {
    pub open: bool,
    pub root: PathBuf,

    // UI state:
    pub selected: usize,
    pub scroll_offset: usize,

    // Filtering:
    pub show_hidden: bool,
    pub show_ignored: bool,

    // Computed view (UI model):
    #[serde(skip)]
    pub rows: Vec<FileTreeRow>,

    // Expanded directories (maintained by the TUI runtime).
    #[serde(skip)]
    pub expanded_dirs: HashSet<PathBuf>,

    // Async loading + cache (maintained by the TUI runtime):
    #[serde(skip)]
    pub loading_dirs: HashSet<PathBuf>,
    #[serde(skip)]
    pub cache: HashMap<PathBuf, Vec<DirEntryModel>>,
}

impl Default for FileTreeState {
    fn default() -> Self {
        Self {
            open: false,
            root: PathBuf::new(),
            selected: 0,
            scroll_offset: 0,
            show_hidden: false,
            show_ignored: false,
            rows: Vec::new(),
            expanded_dirs: HashSet::new(),
            loading_dirs: HashSet::new(),
            cache: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AppState {
    pub app_kind: AppKind,
    pub workspace_root: PathBuf,
    pub buffers: Vec<Buffer>,
    pub active_editor_buffer_id: usize,
    pub notes_buffer_id: usize,
    pub agents: AgentsState,
    pub mode: Mode,
    pub focus: PaneId,
    pub logs: LogBuffer,
    pub job: JobState,
    pub visualizer: VisualizerState,
    pub metrics: Metrics,
    pub prompt: Option<Prompt>,
    pub show_help: bool,
    pub status: Option<String>,
    pub settings: Settings,
    pub debug: bool,
    pub gol_rule_selected: SelectedRule,
    pub games: GamesState,
    pub file_tree: FileTreeState,
    pub fuzzy_search: FuzzySearchState,
    #[serde(skip)]
    pub yank: Option<String>,
    #[serde(skip)]
    pub yank_kind: YankKind,
    #[serde(skip)]
    pub command_line: Option<CommandLine>,
    #[serde(skip)]
    pub ui_selection: Option<UiSelection>,
    #[serde(skip)]
    pub help_scroll: usize,
    #[serde(skip)]
    pub logs_scroll: usize,
    #[serde(skip)]
    pub syntax_status: String,
    #[serde(skip)]
    pub syntax_debug: Option<SyntaxDebugInfo>,
    #[serde(skip)]
    pub rule_catalog: RuleCatalog,
    #[serde(skip)]
    pub rule_picker: RulePickerState,
    #[serde(skip)]
    pub protocol_picker: ProtocolPickerState,
    #[serde(skip)]
    pub rule_persistence: crate::rule_config::RulePersistence,
    /// Cached genome reports per file path (runtime-only, loaded from .nit/genome/).
    #[serde(skip)]
    pub genome_reports: HashMap<PathBuf, crate::genome_report::GenomeReport>,
    /// Last genome diff text for inclusion in agent prompts.
    #[serde(skip)]
    pub last_genome_diff: Option<String>,
    /// Quality change direction from the last genome recomputation.
    /// +1 = improved, -1 = degraded, 0 = unchanged.
    #[serde(skip)]
    pub genome_quality_delta: i32,
    /// Baseline genome reports captured before the agent's first turn.
    /// Retries compare against these baselines, not the previous iteration.
    #[serde(skip)]
    pub genome_baselines: HashMap<PathBuf, crate::genome_report::GenomeReport>,
    /// Files modified during each agent's turn (per-agent tracking).
    /// Key: agent_id, Value: set of file paths modified during that agent's turn.
    #[serde(skip)]
    pub genome_turn_modified: HashMap<String, HashSet<PathBuf>>,
    /// Which agents currently have active turns. File attribution is done
    /// by the runners via `FileWrite` events — not by filesystem tracking.
    #[serde(skip)]
    pub genome_turn_active: HashSet<String>,
    /// True when genome computation has been requested but not yet executed.
    #[serde(skip)]
    pub genome_computing: bool,
    /// Consecutive genome retry attempts for the current agent turn.
    /// Reset to 0 when quality improves or stays the same.
    #[serde(skip)]
    pub genome_retry_count: u8,
    /// Rolling count of consecutive turns where quality met or exceeded the
    /// agent's adaptive min tier. Used for adaptive quality thresholds — agents
    /// that consistently hit their tier get pushed toward the next one, up to
    /// Tier V (Replicator).
    #[serde(skip)]
    pub genome_agent_streak: HashMap<String, u8>,
    /// Effective minimum tier per agent, elevated by adaptive thresholds.
    #[serde(skip)]
    pub genome_agent_min_tier: HashMap<String, crate::genome_report::GenomeTier>,
    /// Real-time per-file shadow evaluation results during an active agent turn.
    /// Populated by the file watcher as files change; cleared on TurnStarted.
    #[serde(skip)]
    pub genome_shadow_evals: HashMap<PathBuf, GenomeShadowEval>,
    /// Number of authoritative genome evaluations still in flight (background threads).
    /// When this reaches 0 after a TurnCompleted, the retry decision is made.
    #[serde(skip)]
    pub genome_eval_pending: usize,
    /// Running worst delta across in-flight turn evaluations.
    #[serde(skip)]
    pub genome_eval_worst_delta: i32,
    /// Agent ID for the in-flight turn evaluation batch (for retry dispatch).
    #[serde(skip)]
    pub genome_eval_agent_id: Option<String>,
    /// Mission ID for the in-flight turn evaluation batch.
    #[serde(skip)]
    pub genome_eval_mission_id: Option<String>,
    /// Scroll offset for the gate monitor / structural quality pane.
    #[serde(skip)]
    pub gate_monitor_scroll: usize,
    /// Active sub-view for the structural quality pane: Stats or FileScores.
    #[serde(skip)]
    pub gate_monitor_sub_view: GateMonitorSubView,
}

/// Sub-view toggle for the CODE STRUCTURAL QUALITY pane.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum GateMonitorSubView {
    #[default]
    Stats,
    FileScores,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum YankKind {
    #[default]
    Char,
    Line,
}

pub struct ActionOutcome {
    pub should_exit: bool,
    pub state_changed: bool,
}

impl AppState {
    pub fn new(workspace_root: PathBuf, editor: Buffer, notes: Buffer) -> Self {
        let settings = Settings::default();
        let rule_catalog = RuleCatalog::default();
        let gol_rule_selected = SelectedRule::default();
        let file_tree = FileTreeState {
            root: workspace_root.clone(),
            ..FileTreeState::default()
        };
        let fuzzy_search = FuzzySearchState {
            root: workspace_root.clone(),
            ..FuzzySearchState::default()
        };
        Self {
            app_kind: AppKind::Gol,
            workspace_root,
            buffers: vec![editor, notes],
            active_editor_buffer_id: 0,
            notes_buffer_id: 1,
            agents: AgentsState::default(),
            mode: Mode::Normal,
            focus: PaneId::Editor,
            logs: LogBuffer::new(DEFAULT_LOG_CAPACITY),
            job: JobState {
                paused: false,
                progress: 0.0,
            },
            visualizer: VisualizerState {
                seed: 1,
                variant: 0,
                mode: VisualizerMode::SimOnly,
                seed_encoder: SeedEncoderId::TokenSpectrum,
                seed_view: SeedViewMode::Genome,
                seed_plate_mode: SeedPreviewMode::Solid,
                seed_params: SeedParams::default(),
                seed_stats: SeedStats::default(),
                seed_hash: 0,
                input_hash: 0,
                seed_search_active: false,
                seed_search_rps: 0,
                render_mode: GolRenderMode::HalfBlock,
                running: false,
                age_shading: true,
                trails: true,
                overlay_bbox: false,
                overlay_heat: false,
                scanlines: false,
                paused: false,
                paused_by_attractor: false,
                wrap: settings.gol.wrap,
                rule: "B3/S23".to_string(),
                rule_mode: RuleMode::Fixed(RuleRef {
                    id: None,
                    rule: Rule::conway(),
                    name: None,
                }),
                protocol_name: None,
                generation: 0,
                alive: 0,
                period: None,
                auto_stop_policy: AutoStopPolicy::Fixed,
                last_attractor: None,
                tick_ms: settings.gol.tick_ms,
                seed_source: settings.gol.seed_source,
                search_rps: 0,
                leaderboard: Vec::new(),
                last_score: None,
                seed_show_grid: false,
                seed_show_bbox: false,
                seed_show_halo: true,
                seed_show_components: false,
                seed_show_inset: true,
                seed_scanline: false,
                seed_zoom: 1,
                inspector_enabled: true,
                inspect_ascii_x: 0,
                inspect_ascii_y: 0,
                inspect_lifehash_x: 0,
                inspect_lifehash_y: 0,
                inspect_hilbert_x: 0,
                inspect_hilbert_y: 0,
                inspect_ascii_hash: 0,
                inspect_lifehash_hash: 0,
                inspect_hilbert_hash: 0,
                seed_snapshots_written: 0,
                seed_snapshots_dropped: 0,
                seed_snapshot_queue_depth: 0,
                seed_last_snapshot_path: None,
                snapshots_written: 0,
                snapshots_dropped: 0,
                snapshot_queue_depth: 0,
                last_snapshot_path: None,
                petri_hidden: false,
                pending_reseed: false,
                pending_apply: false,
                pending_snapshot: false,
                pending_run: false,
                pending_close: false,
                pending_hide: false,
                pending_show: false,
                pending_rule_change: false,
            },
            metrics: Metrics {
                last_render_ms: 0,
                frame_count: 0,
                last_action: None,
            },
            prompt: None,
            show_help: false,
            status: None,
            settings,
            debug: false,
            gol_rule_selected,
            games: GamesState {
                status: GamesStatus::Idle,
                running: false,
                paused: false,
                petri_hidden: false,
                steps_per_tick: 1,
                steps_use_match_units: false,
                last_error: None,
                runtime: nit_games::RuntimeAcceleratorStats::default(),
                last_run: None,
                last_run_path: None,
                last_event_path: None,
                last_history_path: None,
                analysis: GamesAnalysisState {
                    open: false,
                    running: false,
                    source_path: None,
                    last_error: None,
                    summary: None,
                    preview: None,
                    scroll_offset: 0,
                },
                petri_lines: Vec::new(),
                pending_run: false,
                pending_run_override: None,
                config_preview: None,
                config_preview_pending: false,
                pending_family_run: None,
                family_building: false,
                pending_close: false,
                pending_hide: false,
                pending_show: false,
                pending_export: false,
                pending_analyze: None,
                pending_run_browser: false,
                pending_run_load: None,
                pending_replay: None,
                run_browser: GamesRunBrowserState::default(),
                replay: GamesReplayState::default(),
                strategy_inspect: GamesStrategyInspectState::default(),
                tm_sim: GamesTmSimState::default(),
                ca_sim: GamesCaSimState::default(),
                match_history: GamesMatchHistoryState::default(),
            },
            file_tree,
            fuzzy_search,
            yank: None,
            yank_kind: YankKind::Char,
            command_line: None,
            ui_selection: None,
            help_scroll: 0,
            logs_scroll: 0,
            syntax_status: String::new(),
            syntax_debug: None,
            rule_catalog,
            rule_picker: RulePickerState::default(),
            protocol_picker: ProtocolPickerState::default(),
            rule_persistence: crate::rule_config::RulePersistence::default(),
            genome_reports: HashMap::new(),
            last_genome_diff: None,
            genome_computing: false,
            genome_quality_delta: 0,
            genome_baselines: HashMap::new(),
            genome_turn_modified: HashMap::new(),
            genome_turn_active: HashSet::new(),
            genome_retry_count: 0,
            genome_agent_streak: HashMap::new(),
            genome_agent_min_tier: HashMap::new(),
            genome_shadow_evals: HashMap::new(),
            genome_eval_pending: 0,
            genome_eval_worst_delta: 0,
            genome_eval_agent_id: None,
            genome_eval_mission_id: None,
            gate_monitor_scroll: 0,
            gate_monitor_sub_view: GateMonitorSubView::default(),
        }
    }

    pub fn init_rules(
        &mut self,
        rule_catalog: RuleCatalog,
        selected: SelectedRule,
        persistence: crate::rule_config::RulePersistence,
    ) {
        self.rule_catalog = rule_catalog;
        self.rule_persistence = persistence;
        let _ = self.set_gol_rule(selected, false);
        self.visualizer.pending_rule_change = false;
    }

    pub fn set_gol_rule(&mut self, selected: SelectedRule, persist: bool) -> Result<bool, String> {
        let changed = self.gol_rule_selected.rule != selected.rule;
        self.gol_rule_selected = selected;
        self.visualizer.rule = self.gol_rule_selected.rule.to_string();
        self.visualizer.rule_mode =
            RuleMode::Fixed(RuleRef::from_selected(&self.gol_rule_selected));
        self.visualizer.protocol_name = None;
        if changed {
            self.visualizer.pending_rule_change = true;
        }
        if persist {
            let canonical = self.gol_rule_selected.rule.to_string();
            crate::persist_rule_selection(&self.rule_persistence, &canonical)
                .map_err(|err| err.to_string())?;
        }
        Ok(changed)
    }

    pub fn buffer_mut(&mut self, id: usize) -> Option<&mut Buffer> {
        self.buffers.get_mut(id)
    }

    pub fn buffer(&self, id: usize) -> Option<&Buffer> {
        self.buffers.get(id)
    }

    pub fn editor_buffer(&self) -> &Buffer {
        &self.buffers[self.active_editor_buffer_id]
    }

    pub fn editor_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active_editor_buffer_id]
    }

    pub fn notes_buffer(&self) -> &Buffer {
        &self.buffers[self.notes_buffer_id]
    }

    pub fn notes_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.notes_buffer_id]
    }

    pub fn has_unsaved_editor_buffers(&self) -> bool {
        self.buffers
            .iter()
            .enumerate()
            .any(|(id, buf)| id != self.notes_buffer_id && buf.is_dirty())
    }

    fn find_editor_buffer_by_path(&self, path: &Path) -> Option<usize> {
        self.buffers.iter().enumerate().find_map(|(id, buf)| {
            (id != self.notes_buffer_id && buf.path().is_some_and(|buf_path| buf_path == path))
                .then_some(id)
        })
    }

    pub fn focused_buffer_mut(&mut self) -> Option<&mut Buffer> {
        match self.focus {
            PaneId::Editor => Some(self.editor_buffer_mut()),
            PaneId::Notes => Some(self.notes_buffer_mut()),
            PaneId::JobOutput if self.agents.dock_tab == AgentOpsTab::Scratchpad => {
                Some(self.notes_buffer_mut())
            }
            _ => None,
        }
    }

    pub fn set_viewport(&mut self, pane: PaneId, viewport: Viewport) {
        match pane {
            PaneId::Editor => {
                let buf = self.editor_buffer_mut();
                buf.viewport = viewport;
            }
            PaneId::Notes => {
                let buf = self.notes_buffer_mut();
                buf.viewport = viewport;
            }
            _ => {}
        }
    }

    pub fn line_col(&self) -> (usize, usize) {
        let buf = self.editor_buffer();
        (buf.cursor.line + 1, buf.cursor.col + 1)
    }

    pub fn receive_log(&mut self, line: impl Into<String>) {
        self.logs.push(line);
        if self.job.paused {
            // When paused, keep the currently visible log window stable by increasing the
            // "scroll from bottom" offset as new lines arrive.
            self.logs_scroll = self.logs_scroll.saturating_add(1);
        }
    }

    pub fn tick_job(&mut self, delta: f32) {
        if self.job.paused {
            return;
        }
        self.job.progress += delta;
        if self.job.progress >= 1.0 {
            self.job.progress = 0.0;
        }
    }
}

fn focus_order_index(focus: PaneId) -> usize {
    PaneId::ALL.iter().position(|p| *p == focus).unwrap_or(0)
}

pub fn apply_action(state: &mut AppState, action: Action) -> ActionOutcome {
    state.metrics.last_action = Some(action.clone());
    let mut should_exit = false;
    let changed = true;

    match action {
        Action::Quit => {
            if state.has_unsaved_editor_buffers() {
                state.prompt = Some(Prompt::ConfirmQuit);
            } else {
                should_exit = true;
            }
        }
        Action::ConfirmQuitYes => {
            should_exit = true;
        }
        Action::ConfirmQuitNo => {
            state.prompt = None;
        }
        Action::Save | Action::SaveAndNormal => {
            let buf = state.editor_buffer_mut();
            if buf.path().is_none() {
                state.status = Some("No path to save".into());
            } else if let Err(e) = io::save_buffer(buf) {
                state.status = Some(format!("Save failed: {e}"));
            } else {
                buf.mark_clean();
                state.status = Some("Saved".into());
                // Recompute genome report for the saved file.
                if let Some(file_path) = state.editor_buffer().path().cloned() {
                    let text = state.editor_buffer().content_as_string();
                    let report = crate::genome_report::compute_genome_report(&text, &file_path);
                    let (msg, delta) = if let Some(prev) = state.genome_reports.get(&file_path) {
                        let gen_before: i32 = prev
                            .encoder_scores
                            .iter()
                            .map(|s| s.generations_survived as i32)
                            .sum();
                        let gen_after: i32 = report
                            .encoder_scores
                            .iter()
                            .map(|s| s.generations_survived as i32)
                            .sum();
                        let d = gen_after - gen_before;
                        let diff = crate::genome_report::compute_genome_diff(prev, &report);
                        if diff.tier_after > diff.tier_before {
                            (
                                format!(
                                    "Saved \u{2014} quality upgraded: {} \u{2192} {}",
                                    diff.tier_before, diff.tier_after,
                                ),
                                1,
                            )
                        } else if diff.tier_after < diff.tier_before {
                            (
                                format!(
                                    "Saved \u{2014} quality degraded: {} \u{2192} {}",
                                    diff.tier_before, diff.tier_after,
                                ),
                                -1,
                            )
                        } else if d > 0 {
                            (format!("Saved \u{2014} quality improved (+{d} gen)"), 1)
                        } else if d < 0 {
                            (format!("Saved \u{2014} quality declined ({d} gen)"), -1)
                        } else {
                            (
                                format!("Saved \u{2014} quality unchanged ({})", report.tier),
                                0,
                            )
                        }
                    } else {
                        (format!("Saved \u{2014} genome: {}", report.tier), 0)
                    };
                    state.genome_quality_delta = delta;
                    state.genome_reports.insert(file_path, report);
                    state.status = Some(msg);
                }
            }
            if matches!(action, Action::SaveAndNormal) {
                state.mode = Mode::Normal;
                if let Some(buf) = state.focused_buffer_mut() {
                    buf.exit_insert_mode();
                    buf.clear_selection();
                }
            }
        }
        Action::FocusNextPane => {
            let idx = focus_order_index(state.focus);
            let next = (idx + 1) % PaneId::ALL.len();
            state.focus = PaneId::ALL[next];
        }
        Action::FocusPrevPane => {
            let idx = focus_order_index(state.focus);
            let prev = if idx == 0 {
                PaneId::ALL.len() - 1
            } else {
                idx - 1
            };
            state.focus = PaneId::ALL[prev];
        }
        Action::FocusPane(p) => {
            state.focus = p;
        }
        Action::SwitchMode(m) => {
            state.mode = m;
            if let Some(buf) = state.focused_buffer_mut() {
                if m == Mode::Normal {
                    buf.exit_insert_mode();
                    buf.clear_selection();
                } else if m == Mode::Visual {
                    buf.set_selection_anchor();
                } else {
                    buf.clear_selection();
                }
            }
        }
        Action::ToggleMode => {
            state.mode = state.mode.toggle();
            let mode = state.mode;
            if let Some(buf) = state.focused_buffer_mut() {
                if mode == Mode::Normal {
                    buf.exit_insert_mode();
                }
                buf.clear_selection();
            }
        }
        Action::InsertChar(c) => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.insert_char(c);
                buf.ensure_visible();
            }
        }
        Action::InsertNewline => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.insert_newline();
                buf.ensure_visible();
            }
        }
        Action::InsertTab => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.insert_tab();
                buf.ensure_visible();
            }
        }
        Action::EnterVisual => {
            state.mode = Mode::Visual;
            if let Some(buf) = state.focused_buffer_mut() {
                buf.set_selection_anchor();
            }
        }
        Action::ExitVisual => {
            state.mode = Mode::Normal;
            if let Some(buf) = state.focused_buffer_mut() {
                buf.clear_selection();
            }
        }
        Action::YankSelection => {
            let yank = if let Some(buf) = state.focused_buffer_mut() {
                let yank = buf.yank_selection();
                buf.clear_selection();
                yank
            } else {
                None
            };
            if let Some(text) = yank {
                state.yank_kind = if text.contains('\n') {
                    YankKind::Line
                } else {
                    YankKind::Char
                };
                state.yank = Some(text);
            } else {
                state.yank = None;
                state.yank_kind = YankKind::Char;
            }
            state.mode = Mode::Normal;
        }
        Action::YankLine => {
            if let Some(buf) = state.focused_buffer_mut() {
                state.yank = Some(buf.yank_line());
                state.yank_kind = YankKind::Line;
            }
        }
        Action::DeleteSelection => {
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.delete_selection() {
                    buf.ensure_visible();
                }
            }
            state.mode = Mode::Normal;
        }
        Action::Paste => {
            let yank = state.yank.clone();
            let is_normal = state.mode == Mode::Normal;
            let yank_kind = state.yank_kind;
            if let (Some(yank), Some(buf)) = (yank, state.focused_buffer_mut()) {
                if is_normal && yank_kind == YankKind::Line {
                    buf.paste_line_below(&yank);
                } else {
                    if is_normal {
                        buf.append();
                    }
                    buf.insert_str(&yank);
                }
                buf.ensure_visible();
            }
        }
        Action::PasteLineAbove => {
            let yank = state.yank.clone();
            let yank_kind = state.yank_kind;
            if let (Some(yank), Some(buf)) = (yank, state.focused_buffer_mut()) {
                if yank_kind == YankKind::Line {
                    buf.paste_line_above(&yank);
                } else {
                    let mut text = yank;
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                    buf.paste_line_above(&text);
                }
                buf.ensure_visible();
            }
        }
        Action::Append => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.append();
                buf.ensure_visible();
                state.mode = Mode::Insert;
            }
        }
        Action::Backspace => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.backspace();
                buf.ensure_visible();
            }
        }
        Action::Delete => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.delete_forward();
                buf.ensure_visible();
            }
        }
        Action::DeleteLine => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.delete_line();
                buf.ensure_visible();
            }
        }
        Action::MoveUp => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_up();
                buf.ensure_visible();
            }
        }
        Action::MoveDown => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_down();
                buf.ensure_visible();
            }
        }
        Action::MoveLeft => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_left();
                buf.ensure_visible();
            }
        }
        Action::MoveRight => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_right();
                buf.ensure_visible();
            }
        }
        Action::PageUp => {
            if let Some(buf) = state.focused_buffer_mut() {
                let height = buf.viewport.height.max(1);
                buf.page_up(height);
                buf.ensure_visible();
            }
        }
        Action::PageDown => {
            if let Some(buf) = state.focused_buffer_mut() {
                let height = buf.viewport.height.max(1);
                buf.page_down(height);
                buf.ensure_visible();
            }
        }
        Action::Home => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_home();
                buf.ensure_visible();
            }
        }
        Action::End => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_end();
                buf.ensure_visible();
            }
        }
        Action::MoveWordEnd => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_word_end();
                buf.ensure_visible();
            }
        }
        Action::MoveWordBack => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_word_back();
                buf.ensure_visible();
            }
        }
        Action::GoToTop => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.go_to_top();
                buf.ensure_visible();
            }
        }
        Action::GoToBottom => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.go_to_bottom();
                buf.ensure_visible();
            }
        }
        Action::OpenLineAbove => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.open_line_above();
                buf.ensure_visible();
                state.mode = Mode::Insert;
            }
        }
        Action::OpenLineBelow => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.open_line_below();
                buf.ensure_visible();
                state.mode = Mode::Insert;
            }
        }
        Action::Undo => {
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.undo() {
                    buf.ensure_visible();
                }
            }
        }
        Action::Redo => {
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.redo() {
                    buf.ensure_visible();
                }
            }
        }
        Action::ScrollUp => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.viewport.offset_line = buf.viewport.offset_line.saturating_sub(1);
            }
        }
        Action::ScrollDown => {
            if let Some(buf) = state.focused_buffer_mut() {
                let max_offset = buf.lines_len().saturating_sub(buf.viewport.height.max(1));
                buf.viewport.offset_line = buf.viewport.offset_line.saturating_add(1).min(max_offset);
            }
        }
        Action::ClearLogs => {
            state.logs.clear();
            state.logs_scroll = 0;
        }
        Action::ToggleJobPause => {
            let was_paused = state.job.paused;
            state.job.paused = !state.job.paused;
            if was_paused {
                // Resume log follow.
                state.logs_scroll = 0;
            }
        }
        Action::CommandPromptOpen => {
            state.command_line = Some(CommandLine::new());
        }
        Action::CommandPromptCancel => {
            state.command_line = None;
        }
        Action::CommandPromptBackspace => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.backspace();
            }
        }
        Action::CommandPromptMoveLeft => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.move_left();
            }
        }
        Action::CommandPromptMoveRight => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.move_right();
            }
        }
        Action::CommandPromptExecute => {
            if let Some(cmd) = state.command_line.take() {
                should_exit = handle_command_line(state, &cmd.input);
            }
        }
        Action::CommandPromptInput(ch) => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.insert(ch);
            }
        }
        Action::VisualizerReseed => {
            state.visualizer.seed = state.visualizer.seed.wrapping_add(1);
            state.visualizer.pending_reseed = true;
        }
        Action::VisualizerApply => {
            if state.visualizer.seed_search_active {
                state.visualizer.pending_apply = true;
            } else {
                state.visualizer.variant = state.visualizer.variant.wrapping_add(1);
                state.visualizer.pending_reseed = true;
            }
        }
        Action::VisualizerToggleSearch => {
            state.visualizer.seed_search_active = !state.visualizer.seed_search_active;
            state.status = Some(if state.visualizer.seed_search_active {
                "Seed search ON".into()
            } else {
                "Seed search OFF".into()
            });
        }
        Action::VisualizerToggleWrap => {
            state.visualizer.wrap = !state.visualizer.wrap;
        }
        Action::VisualizerToggleSeedSource => {
            state.visualizer.seed_source = GolSeedSource::Editor;
            state.status = Some("Seed source: Editor (only)".into());
        }
        Action::VisualizerSnapshot => {
            state.visualizer.pending_snapshot = true;
        }
        Action::VisualizerPause => {
            state.visualizer.paused = !state.visualizer.paused;
            state.visualizer.paused_by_attractor = false;
        }
        Action::VisualizerCycleAutoStop => {
            state.visualizer.auto_stop_policy = state.visualizer.auto_stop_policy.next();
            state.status = Some(format!("Auto-stop: {}", state.visualizer.auto_stop_policy));
        }
        Action::VisualizerSpeedUp => {
            state.visualizer.tick_ms = state.visualizer.tick_ms.saturating_sub(10).max(30);
        }
        Action::VisualizerSpeedDown => {
            state.visualizer.tick_ms = (state.visualizer.tick_ms + 10).min(1000);
        }
        Action::VisualizerRun => {
            state.visualizer.pending_run = true;
            state.visualizer.pending_snapshot = true;
            state.status = Some("Petri dish queued".into());
        }
        Action::VisualizerStop => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
        }
        Action::GamesRun => {
            state.games.pending_run = true;
            state.status = Some("Games tournament queued".into());
        }
        Action::GamesStop => {
            state.games.pending_close = true;
            state.status = Some("Games tournament closing".into());
        }
        Action::GamesHide => {
            state.games.pending_hide = true;
            state.status = Some("Games tournament hiding".into());
        }
        Action::GamesShow => {
            state.games.pending_show = true;
            state.status = Some("Games tournament showing".into());
        }
        Action::GamesHistoryOpen => {
            open_games_history_popup(state);
        }
        Action::PetriShow => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
        }
        Action::VisualizerCycleRenderMode => {
            state.visualizer.seed_plate_mode = state.visualizer.seed_plate_mode.next();
            state.status = Some(format!(
                "Plate mode: {}",
                state.visualizer.seed_plate_mode.label()
            ));
        }
        Action::VisualizerToggleAgeShading => {
            state.visualizer.age_shading = !state.visualizer.age_shading;
            state.status = Some(format!(
                "Age shading: {}",
                if state.visualizer.age_shading {
                    "ON"
                } else {
                    "OFF"
                }
            ));
        }
        Action::VisualizerToggleTrails => {
            state.visualizer.trails = !state.visualizer.trails;
            state.status = Some(format!(
                "Trails: {}",
                if state.visualizer.trails { "ON" } else { "OFF" }
            ));
        }
        Action::VisualizerToggleBBox => {
            state.visualizer.overlay_bbox = !state.visualizer.overlay_bbox;
            state.status = Some(format!(
                "BBox: {}",
                if state.visualizer.overlay_bbox {
                    "ON"
                } else {
                    "OFF"
                }
            ));
        }
        Action::VisualizerToggleHeat => {
            state.visualizer.overlay_heat = !state.visualizer.overlay_heat;
            state.status = Some(format!(
                "Heat: {}",
                if state.visualizer.overlay_heat {
                    "ON"
                } else {
                    "OFF"
                }
            ));
        }
        Action::VisualizerToggleScanlines => {
            state.visualizer.scanlines = !state.visualizer.scanlines;
            state.status = Some(format!(
                "Scanlines: {}",
                if state.visualizer.scanlines {
                    "ON"
                } else {
                    "OFF"
                }
            ));
        }
        Action::GateMonitorToggleSubView => {
            state.gate_monitor_sub_view = match state.gate_monitor_sub_view {
                GateMonitorSubView::Stats => GateMonitorSubView::FileScores,
                GateMonitorSubView::FileScores => GateMonitorSubView::Stats,
            };
            state.gate_monitor_scroll = 0;
        }
        Action::VisualizerCycleSeedView => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
        }
        Action::VisualizerToggleSeedView => {
            state.visualizer.seed_view = state.visualizer.seed_view.toggle_plate();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
        }
        Action::VisualizerCycleSeedOverlays => {
            cycle_seed_overlays(&mut state.visualizer);
            state.status = Some(format!(
                "Overlays: {}",
                seed_overlay_label(&state.visualizer)
            ));
        }
        Action::VisualizerInspectLeft => {
            move_inspector(state, -1, 0);
        }
        Action::VisualizerInspectRight => {
            move_inspector(state, 1, 0);
        }
        Action::VisualizerInspectUp => {
            move_inspector(state, 0, -1);
        }
        Action::VisualizerInspectDown => {
            move_inspector(state, 0, 1);
        }
        Action::VisualizerInspectHome => {
            set_inspector_pos(state, 0, 0);
        }
        Action::VisualizerInspectEnd => {
            let (w, h) = inspector_dims(state);
            if w > 0 && h > 0 {
                set_inspector_pos(state, w - 1, h - 1);
            }
        }
        Action::VisualizerInspectCenter => {
            let (w, h) = inspector_dims(state);
            if w > 0 && h > 0 {
                set_inspector_pos(state, w / 2, h / 2);
            }
        }
        Action::VisualizerInspectToggle => {
            state.visualizer.inspector_enabled = !state.visualizer.inspector_enabled;
            state.status = Some(format!(
                "Inspector: {}",
                if state.visualizer.inspector_enabled {
                    "ON"
                } else {
                    "OFF"
                }
            ));
        }
        Action::VisualizerInspectJump(idx) => {
            jump_inspector_to_index(state, idx);
        }
        Action::VisualizerCycleEncoder => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
        }
        Action::VisualizerCycleSymmetry => {
            state.visualizer.seed_params.symmetry = state.visualizer.seed_params.symmetry.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Symmetry: {}",
                state.visualizer.seed_params.symmetry.label()
            ));
        }
        Action::SetGolRuleById(id) => {
            if let Some(named) = state.rule_catalog.find_by_id(&id) {
                apply_rule_selection(state, SelectedRule::from_named(named), true);
            } else {
                state.status = Some(format!("Unknown GoL rule id: {id}"));
            }
        }
        Action::SetGolRuleByString(text) => match Rule::parse(&text) {
            Ok(rule) => {
                let mut selected = SelectedRule::from_rule(rule);
                if let Some(named) = state.rule_catalog.find_by_rule(rule) {
                    selected.id = Some(named.id.clone());
                    selected.name = Some(named.name.clone());
                }
                apply_rule_selection(state, selected, true);
            }
            Err(err) => {
                state.status = Some(format!("Invalid GoL rule '{text}': {err}"));
            }
        },
        Action::OpenRulePicker => {
            if matches!(state.visualizer.rule_mode, RuleMode::Protocol(_)) {
                state.status = Some("Rule picker disabled in protocol mode".into());
            } else {
                state.rule_picker.open = true;
                state.rule_picker.query.clear();
                state.rule_picker.selected = state
                    .rule_catalog
                    .index_of_selected(&state.gol_rule_selected)
                    .unwrap_or(0);
            }
        }
        Action::OpenProtocolPicker => {
            state.protocol_picker.open = true;
            state.protocol_picker.selected = 0;
            state.protocol_picker.custom_input.clear();
            state.protocol_picker.custom_error = None;
            state.protocol_picker.custom_preview = None;
        }
        Action::CloseModal => {
            state.rule_picker.open = false;
            state.rule_picker.query.clear();
            state.rule_picker.selected = 0;
            state.protocol_picker.open = false;
            state.protocol_picker.custom_error = None;
            state.protocol_picker.custom_preview = None;
        }
        Action::ApplySelectedRuleFromPicker => {
            let matches = state.rule_catalog.filter_indices(&state.rule_picker.query);
            if matches.is_empty() {
                state.status = Some("No rules match filter".into());
                state.rule_picker.open = false;
            } else {
                let idx = state
                    .rule_picker
                    .selected
                    .min(matches.len().saturating_sub(1));
                if let Some(named) = state.rule_catalog.get(matches[idx]) {
                    apply_rule_selection(state, SelectedRule::from_named(named), true);
                }
                state.rule_picker.open = false;
            }
        }
        Action::ApplySelectedProtocolFromPicker => {
            let presets = crate::rule_protocol::builtin_protocols(&state.rule_catalog);
            let idx = state
                .protocol_picker
                .selected
                .min(presets.len().saturating_add(1).saturating_sub(1));
            if idx < presets.len() {
                let preset = &presets[idx];
                apply_protocol_selection(state, preset.mode.clone(), Some(preset.name.clone()));
                state.status = Some(format!("Protocol set to {}", preset.name));
                state.protocol_picker.open = false;
                state.protocol_picker.custom_error = None;
            } else {
                match crate::rule_protocol::parse_protocol_spec(
                    &state.protocol_picker.custom_input,
                    &state.rule_catalog,
                ) {
                    Ok(mut protocol) => {
                        protocol.reset();
                        apply_protocol_selection(
                            state,
                            RuleMode::Protocol(protocol),
                            Some("Custom".into()),
                        );
                        state.status = Some("Protocol set to Custom".into());
                        state.protocol_picker.open = false;
                        state.protocol_picker.custom_error = None;
                    }
                    Err(err) => {
                        state.protocol_picker.custom_error = Some(err);
                    }
                }
            }
        }
        Action::ToggleSyntax => {
            state.settings.highlight.enabled = !state.settings.highlight.enabled;
        }
        Action::ToggleDebug => {
            state.debug = !state.debug;
            state.status = Some(if state.debug {
                "Debug ON".into()
            } else {
                "Debug OFF".into()
            });
        }
        Action::ToggleFileTree => {
            state.file_tree.open = !state.file_tree.open;
            if state.file_tree.open {
                state.file_tree.root = state.workspace_root.clone();
                state.focus = PaneId::Editor;
                state.mode = Mode::Normal;
            }
        }
        Action::OpenSearchPopup(mode) => {
            state.show_help = false;
            state.rule_picker.open = false;
            state.protocol_picker.open = false;
            state.fuzzy_search.open(mode, state.workspace_root.clone());
            state.focus = PaneId::Editor;
            state.mode = Mode::Normal;
        }
        Action::CloseSearchPopup => {
            state.fuzzy_search.close();
        }
        Action::OpenFile(path) => {
            if let Some(buffer_id) = state.find_editor_buffer_by_path(&path) {
                state.active_editor_buffer_id = buffer_id;
                state.focus = PaneId::Editor;
                state.mode = Mode::Normal;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Opened {}", path.display()));
            } else {
                match io::load_to_string(&path) {
                    Ok(content) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "untitled".into());
                        let buf = Buffer::from_str(name, &content, Some(path.clone()));
                        if state.editor_buffer().is_dirty() {
                            state.buffers.push(buf);
                            state.active_editor_buffer_id = state.buffers.len() - 1;
                        } else {
                            state.buffers[state.active_editor_buffer_id] = buf;
                        }
                        state.focus = PaneId::Editor;
                        state.mode = Mode::Normal;
                        state.visualizer.pending_reseed = true;
                        state.status = Some(format!("Opened {}", path.display()));
                    }
                    Err(err) => {
                        state.status = Some(format!("Open failed: {err}"));
                    }
                }
            }
        }
        Action::ShowHelp => {
            state.show_help = true;
            state.help_scroll = 0;
        }
        Action::HideHelp => {
            state.show_help = false;
            state.help_scroll = 0;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::HelpPopup) {
                    state.ui_selection = None;
                }
            }
        }
    }

    ActionOutcome {
        should_exit,
        state_changed: changed,
    }
}

fn open_games_history_popup(state: &mut AppState) {
    if state.games.match_history.total_entries == 0 && !state.games.match_history.entries.is_empty()
    {
        state.games.match_history.total_entries = state.games.match_history.entries.len();
        state.games.match_history.loaded_start = 0;
        state.games.match_history.max_rounds_seen = state
            .games
            .match_history
            .entries
            .iter()
            .map(|entry| entry.rounds_total as usize)
            .max()
            .unwrap_or(0);
    }
    if state.games.match_history.total_entries == 0 {
        state.games.match_history.last_error = if state.games.match_history.capture_disabled_for_run
        {
            None
        } else {
            Some("No completed matches available yet. Start a tournament first.".into())
        };
    } else {
        state.games.match_history.last_error = None;
    }
    state.games.run_browser.open = false;
    state.games.replay.open = false;
    state.games.strategy_inspect.open = false;
    state.games.analysis.open = false;
    state.games.tm_sim.open = false;
    state.games.ca_sim.open = false;
    state.games.match_history.open = true;
    state.games.match_history.column_offset = 0;
    state.games.match_history.round_limit = None;
    state.status = Some("Games match history plot opened".into());
}

fn handle_command_line(state: &mut AppState, input: &str) -> bool {
    let trimmed = input.trim();
    let cmd = trimmed.trim_start_matches(':').trim().to_lowercase();
    if cmd.is_empty() {
        return false;
    }
    let normalized = cmd
        .split_whitespace()
        .map(|token| token.trim_matches(':'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let tokens: Vec<&str> = normalized;
    if let Some(target_lab) = lab_from_tokens(&tokens) {
        if target_lab != state.app_kind {
            state.status = Some(format!(
                "{} lab not active (current: {}). Use --lab {} to start.",
                target_lab.label(),
                state.app_kind.label(),
                target_lab
            ));
            return false;
        }
    }
    if is_help_command_tokens(&tokens) {
        state.show_help = true;
        state.help_scroll = 0;
        state.status = Some("Help opened".into());
        return false;
    }
    match tokens.as_slice() {
        ["q"] | ["quit"] | ["exit"] => {
            if state.has_unsaved_editor_buffers() {
                state.prompt = Some(Prompt::ConfirmQuit);
                false
            } else {
                true
            }
        }
        ["tree"] | ["nittree"] | ["explore"] => {
            state.file_tree.open = !state.file_tree.open;
            if state.file_tree.open {
                state.file_tree.root = state.workspace_root.clone();
                state.focus = PaneId::Editor;
                state.mode = Mode::Normal;
                state.status = Some("NITTree opened".into());
            } else {
                state.status = Some("NITTree closed".into());
            }
            false
        }
        ["find"] | ["ff"] => {
            state
                .fuzzy_search
                .open(SearchMode::Files, state.workspace_root.clone());
            state.status = Some("Search: files".into());
            false
        }
        ["grep"] | ["rg"] | ["search"] => {
            state
                .fuzzy_search
                .open(SearchMode::Content, state.workspace_root.clone());
            state.status = Some("Search: content".into());
            false
        }
        ["close"] => {
            if state.fuzzy_search.open {
                state.fuzzy_search.close();
                state.status = Some("Search closed".into());
            }
            false
        }
        ["run"] => match state.app_kind {
            AppKind::Gol => {
                state.visualizer.pending_run = true;
                state.visualizer.pending_snapshot = true;
                state.status = Some("Petri dish queued".into());
                false
            }
            AppKind::Games => {
                state.games.pending_run_override = None;
                state.games.pending_family_run = None;
                state.games.family_building = false;
                state.games.pending_run = true;
                state.status = Some("Games tournament queued".into());
                false
            }
        },
        ["gol", "run"] | ["run", "gol"] | ["life", "run"] | ["gol", "start"] | ["run", "life"] => {
            state.visualizer.pending_run = true;
            state.visualizer.pending_snapshot = true;
            state.status = Some("Petri dish queued".into());
            false
        }
        _ if tokens.first() == Some(&"games")
            && tokens.get(1) == Some(&"run")
            && tokens.len() > 2 =>
        {
            let (force, family) = if tokens.get(2) == Some(&"force") {
                match tokens.get(3).copied() {
                    Some(family) => (true, family),
                    None => {
                        state.status = Some(
                            "Usage: :games run force <fsm|ca|tm> {params} (e.g. :games run force fsm {3, 2})"
                                .into(),
                        );
                        return false;
                    }
                }
            } else {
                (false, tokens[2])
            };

            if state.games.family_building {
                state.status = Some("Family run preparation already in progress".into());
                return false;
            }

            match build_family_run_override(state, family, trimmed, force) {
                Ok(request) => {
                    state.games.pending_run_override = None;
                    state.games.pending_run = false;
                    state.games.pending_family_run = Some(request);
                    state.games.family_building = true;
                    let mode = if force { "forced, " } else { "" };
                    state.status = Some(format!("Preparing family run ({mode}{family})..."));
                }
                Err(err) => {
                    state.games.pending_family_run = None;
                    state.games.family_building = false;
                    state.status = Some(err)
                }
            }
            false
        }
        ["games", "run"] | ["run", "games"] => {
            state.games.pending_run_override = None;
            state.games.pending_family_run = None;
            state.games.family_building = false;
            state.games.pending_run = true;
            state.status = Some("Games tournament queued".into());
            false
        }
        ["gol", "hide"] | ["hide", "gol"] => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
            false
        }
        ["gol", "show"] | ["show", "gol"] => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
            false
        }
        ["gol", "stop"] | ["life", "stop"] => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
            false
        }
        ["run", "stop"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
            false
        }
        ["games", "hide"] | ["hide", "games"] => {
            state.games.pending_hide = true;
            state.status = Some("Games tournament hiding".into());
            false
        }
        ["games", "show"] | ["show", "games"] => {
            state.games.pending_show = true;
            state.status = Some("Games tournament showing".into());
            false
        }
        ["games", "stop"] | ["stop", "games"] => {
            state.games.pending_close = true;
            state.status = Some("Games tournament closing".into());
            false
        }
        ["games", "status"] => {
            state.status = Some(format!("Games status: {:?}", state.games.status));
            false
        }
        ["games", "runs"] | ["games", "browse"] | ["games", "browser"] => {
            state.games.replay.open = false;
            state.games.match_history.open = false;
            state.games.run_browser.open = true;
            state.games.run_browser.loading = true;
            state.games.run_browser.last_error = None;
            state.games.run_browser.entries.clear();
            state.games.run_browser.selected = 0;
            state.games.run_browser.scroll_offset = 0;
            state.games.pending_run_browser = true;
            state.status = Some("Games run browser opened".into());
            false
        }
        ["games", "replay"] => {
            if state.games.last_run.is_none() {
                state.status = Some("No run loaded for replay".into());
            } else {
                state.games.run_browser.open = false;
                state.games.match_history.open = false;
                state.games.replay.open = true;
                state.games.replay.loading = false;
                state.games.replay.last_error = None;
                state.games.replay.selected_pair = None;
                state.games.replay.selected_index = 0;
                state.games.replay.title = None;
                state.games.replay.lines.clear();
                state.games.replay.scroll_offset = 0;
                state.games.replay.cycle = None;
                state.status = Some("Games replay opened".into());
            }
            false
        }
        ["games", "history"] | ["games", "hist"] | ["games", "plot"] | ["games", "plots"] => {
            open_games_history_popup(state);
            false
        }
        ["history"] | ["hist"] | ["plot"] | ["plots"] if state.app_kind == AppKind::Games => {
            open_games_history_popup(state);
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"inspect") => {
            let rule_tuple = match parse_tm_rule_tuple(trimmed) {
                Ok(value) => value,
                Err(msg) => {
                    state.status = Some(msg);
                    return false;
                }
            };

            // Allow either:
            // - :games inspect <strategy_id>
            // - :games inspect <fsm_index>                (defaults to {index,2,2})
            // - :games inspect <strategy_id> {rule_code, states, symbols}
            // - :games inspect {rule_code, states, symbols}
            let explicit_target = tokens
                .get(2)
                .copied()
                .filter(|token| !token.starts_with('{'));
            let explicit_fsm_index = explicit_target.and_then(parse_tm_input_token);
            let explicit_id = explicit_target.filter(|token| parse_tm_input_token(token).is_none());

            if rule_tuple.is_none() && explicit_id.is_none() && explicit_fsm_index.is_none() {
                state.status = Some(
                    "Usage: :games inspect <strategy_id> | :games inspect <fsm_index> | :games inspect fsm {index,states,k} | :games inspect <strategy_id> {rule,states,symbols} | :games inspect {rule,states,symbols}"
                        .into(),
                );
                return false;
            }

            let mut spec: Option<nit_games::StrategySpec> = None;
            let mut definition: Option<nit_games::output::StrategyDefinition> = None;
            let mut source_label: Option<String> = None;

            if let Some(index) = explicit_fsm_index {
                let states = 2usize;
                let actions = 2usize;
                let (outputs, transitions) =
                    match nit_games::strategy::decode_fsm_notebook_index(index, states, actions) {
                        Ok(decoded) => decoded,
                        Err(err) => {
                            state.status = Some(format!("FSM rule decode error: {err}"));
                            return false;
                        }
                    };
                let def = nit_games::output::StrategyDefinition {
                    id: format!("fsm_rule_{index}_{states}x{actions}"),
                    name: Some(format!("FSM index {index} ({states}x{actions})")),
                    kind: nit_games::config::StrategySpecKind::Fsm {
                        num_states: states,
                        start_state: 0,
                        outputs,
                        input_mode: Some(nit_games::strategy::InputMode::OpponentLastAction),
                        transitions,
                        index: Some(index),
                    },
                    rng_seed_a: None,
                    rng_seed_b: None,
                };
                spec = Some(nit_games::StrategySpec {
                    id: def.id.clone(),
                    name: def.name.clone(),
                    kind: def.kind.clone(),
                });
                definition = Some(def);
                source_label = Some("rule".into());
            }

            let tuple_prefers_fsm = rule_tuple.is_some()
                && explicit_id
                    .map(|id| strategy_id_prefers_fsm(state, id))
                    .unwrap_or(false);

            if spec.is_none() && tuple_prefers_fsm {
                let (index, states, actions) = rule_tuple.expect("checked is_some");
                let states = states as usize;
                let actions = actions as usize;
                let (outputs, transitions) =
                    match nit_games::strategy::decode_fsm_notebook_index(index, states, actions) {
                        Ok(decoded) => decoded,
                        Err(err) => {
                            state.status = Some(format!("FSM rule decode error: {err}"));
                            return false;
                        }
                    };
                let effective_id = explicit_id
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("fsm_rule_{index}_{states}x{actions}"));
                let def = nit_games::output::StrategyDefinition {
                    id: effective_id,
                    name: Some(format!("FSM index {index} ({states}x{actions})")),
                    kind: nit_games::config::StrategySpecKind::Fsm {
                        num_states: states,
                        start_state: 0,
                        outputs,
                        input_mode: Some(nit_games::strategy::InputMode::OpponentLastAction),
                        transitions,
                        index: Some(index),
                    },
                    rng_seed_a: None,
                    rng_seed_b: None,
                };

                spec = Some(nit_games::StrategySpec {
                    id: def.id.clone(),
                    name: def.name.clone(),
                    kind: def.kind.clone(),
                });
                definition = Some(def);
                source_label = Some("rule".into());
            }

            if spec.is_none() {
                if let Some((rule_code, states, symbols)) = rule_tuple {
                    if states == 0 || symbols < 2 {
                        state.status = Some(if states == 0 {
                            "TM rule tuple: states must be >= 1".into()
                        } else {
                            "TM rule tuple: symbols must be >= 2".into()
                        });
                        return false;
                    }

                    let (transitions, _remaining) =
                        nit_games::strategy::decode_tm_rule_code_wolfram(
                            rule_code,
                            states as usize,
                            symbols as usize,
                        );
                    let output_map: Vec<nit_games::game::Action> = (0..symbols)
                        .map(|idx| {
                            if idx == 0 {
                                nit_games::game::Action::Cooperate
                            } else {
                                nit_games::game::Action::Defect
                            }
                        })
                        .collect();

                    let max_steps = 256;
                    let effective_id = explicit_id
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("tm_rule_{rule_code}_{states}x{symbols}"));
                    let def = nit_games::output::StrategyDefinition {
                        id: effective_id,
                        name: Some(format!("Rule {rule_code} ({states}x{symbols})")),
                        kind: nit_games::config::StrategySpecKind::OneSidedTm {
                            states,
                            symbols,
                            start_state: 1,
                            blank: 0,
                            fallback_symbol: Some(0),
                            max_steps_per_round: max_steps,
                            input_mode: nit_games::strategy::InputMode::OpponentLastAction,
                            output_map,
                            transitions,
                            rule_code: Some(rule_code),
                        },
                        rng_seed_a: None,
                        rng_seed_b: None,
                    };

                    spec = Some(nit_games::StrategySpec {
                        id: def.id.clone(),
                        name: def.name.clone(),
                        kind: def.kind.clone(),
                    });
                    definition = Some(def);
                    source_label = Some("rule".into());
                }
            }

            if let Some(run) = state.games.last_run.as_ref() {
                if spec.is_none() {
                    if let Some(def) = run
                        .strategies
                        .iter()
                        .find(|s| s.id == explicit_id.unwrap_or_default())
                        .cloned()
                    {
                        spec = Some(nit_games::StrategySpec {
                            id: def.id.clone(),
                            name: def.name.clone(),
                            kind: def.kind.clone(),
                        });
                        definition = Some(def);
                        source_label = Some("run".into());
                    }
                }
            }

            if spec.is_none() {
                let target_id = explicit_id.unwrap_or("tm_rule");
                let config_text = state.editor_buffer().content_as_string();
                match nit_games::config::GamesConfig::from_toml_with_root(
                    &config_text,
                    Some(&state.workspace_root),
                ) {
                    Ok(config) => {
                        if let Some(found) = config.strategies.iter().find(|s| s.id == target_id) {
                            spec = Some(found.clone());
                            definition = Some(nit_games::output::StrategyDefinition {
                                id: found.id.clone(),
                                name: found.name.clone(),
                                kind: found.kind.clone(),
                                rng_seed_a: None,
                                rng_seed_b: None,
                            });
                            source_label = Some("config".into());
                        }
                    }
                    Err(err) => {
                        state.games.run_browser.open = false;
                        state.games.replay.open = false;
                        state.games.tm_sim.open = false;
                        state.games.ca_sim.open = false;
                        state.games.analysis.open = false;
                        state.games.strategy_inspect.open = true;
                        state.games.strategy_inspect.last_error =
                            Some(format!("Config error: {err}"));
                        state.games.strategy_inspect.title = None;
                        state.games.strategy_inspect.lines.clear();
                        state.games.strategy_inspect.definition = None;
                        state.games.strategy_inspect.selected_index = 0;
                        state.games.strategy_inspect.scroll_offset = 0;
                        state.games.strategy_inspect.definitions.clear();
                        state.games.strategy_inspect.source_label = Some("config".into());
                        state.status = Some("Games strategy inspect error".into());
                        return false;
                    }
                }
            }

            let Some(spec) = spec else {
                let target_id = explicit_id.unwrap_or("tm_rule");
                state.games.run_browser.open = false;
                state.games.replay.open = false;
                state.games.tm_sim.open = false;
                state.games.ca_sim.open = false;
                state.games.analysis.open = false;
                state.games.strategy_inspect.open = true;
                state.games.strategy_inspect.last_error =
                    Some(format!("Strategy '{target_id}' not found in run or config"));
                state.games.strategy_inspect.title = None;
                state.games.strategy_inspect.lines.clear();
                state.games.strategy_inspect.definition = None;
                state.games.strategy_inspect.selected_index = 0;
                state.games.strategy_inspect.scroll_offset = 0;
                state.games.strategy_inspect.definitions.clear();
                state.games.strategy_inspect.source_label = None;
                state.status = Some("Games strategy inspect error".into());
                return false;
            };

            let intro = nit_games::introspect_strategy(&spec);
            let lines = nit_games::format_strategy_introspection(&intro);

            state.games.run_browser.open = false;
            state.games.replay.open = false;
            state.games.tm_sim.open = false;
            state.games.ca_sim.open = false;
            state.games.analysis.open = false;
            state.games.strategy_inspect.open = true;
            state.games.strategy_inspect.last_error = None;
            state.games.strategy_inspect.title = Some(format!("{} — inspect", spec.id));
            state.games.strategy_inspect.lines = lines;
            state.games.strategy_inspect.definition = definition;
            state.games.strategy_inspect.selected_index = 0;
            state.games.strategy_inspect.scroll_offset = 0;
            state.games.strategy_inspect.definitions.clear();
            state.games.strategy_inspect.source_label = source_label;
            state.status = Some(format!("Games inspect: {}", spec.id));
            false
        }
        ["games", "strategy"]
        | ["games", "strategies"]
        | ["games", "strategy", "run"]
        | ["games", "strategies", "run"] => {
            if state.games.last_run.is_none() {
                state.status = Some("No run loaded for strategy inspection".into());
            } else if let Some(run) = state.games.last_run.as_ref() {
                state.games.run_browser.open = false;
                state.games.replay.open = false;
                state.games.tm_sim.open = false;
                state.games.ca_sim.open = false;
                state.games.strategy_inspect.open = true;
                state.games.strategy_inspect.last_error = None;
                state.games.strategy_inspect.title = None;
                state.games.strategy_inspect.lines.clear();
                state.games.strategy_inspect.definition = None;
                state.games.strategy_inspect.selected_index = 0;
                state.games.strategy_inspect.scroll_offset = 0;
                state.games.strategy_inspect.definitions = run.strategies.clone();
                state.games.strategy_inspect.source_label = Some("run".into());
                state.status = Some("Games strategy inspector opened".into());
            }
            false
        }
        ["games", "strategy", "all"]
        | ["games", "strategies", "all"]
        | ["games", "strategy", "config"]
        | ["games", "strategies", "config"] => {
            let config_text = state.editor_buffer().content_as_string();
            match nit_games::config::GamesConfig::from_toml_with_root(
                &config_text,
                Some(&state.workspace_root),
            ) {
                Ok(config) => {
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.tm_sim.open = false;
                    state.games.ca_sim.open = false;
                    state.games.strategy_inspect.open = true;
                    state.games.strategy_inspect.last_error = None;
                    state.games.strategy_inspect.title = None;
                    state.games.strategy_inspect.lines.clear();
                    state.games.strategy_inspect.definition = None;
                    state.games.strategy_inspect.selected_index = 0;
                    state.games.strategy_inspect.scroll_offset = 0;
                    state.games.strategy_inspect.definitions = config
                        .strategies
                        .iter()
                        .map(|spec| nit_games::output::StrategyDefinition {
                            id: spec.id.clone(),
                            name: spec.name.clone(),
                            kind: spec.kind.clone(),
                            rng_seed_a: None,
                            rng_seed_b: None,
                        })
                        .collect();
                    state.games.strategy_inspect.source_label = Some("config".into());
                    state.status = Some("Games strategy inspector opened".into());
                }
                Err(err) => {
                    let msg = format!("Config error: {err}");
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.tm_sim.open = false;
                    state.games.ca_sim.open = false;
                    state.games.strategy_inspect.open = true;
                    state.games.strategy_inspect.last_error = Some(msg.clone());
                    state.games.strategy_inspect.title = None;
                    state.games.strategy_inspect.lines.clear();
                    state.games.strategy_inspect.definition = None;
                    state.games.strategy_inspect.selected_index = 0;
                    state.games.strategy_inspect.scroll_offset = 0;
                    state.games.strategy_inspect.definitions.clear();
                    state.games.strategy_inspect.source_label = Some("config".into());
                    state.status = Some(msg);
                }
            }
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"tm") => {
            let mut idx = 2usize;
            let mut source = "config";
            if let Some(token) = tokens.get(idx) {
                if *token == "run" {
                    source = "run";
                    idx += 1;
                } else if *token == "config" {
                    source = "config";
                    idx += 1;
                }
            }

            let rule_tuple = match parse_tm_rule_tuple(trimmed) {
                Ok(value) => value,
                Err(msg) => {
                    state.status = Some(msg.clone());
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.strategy_inspect.open = false;
                    state.games.analysis.open = false;
                    state.games.ca_sim.open = false;
                    state.games.tm_sim.open = true;
                    state.games.tm_sim.last_error = Some(msg);
                    state.games.tm_sim.definition = None;
                    state.games.tm_sim.input = None;
                    state.games.tm_sim.steps_override = None;
                    state.games.tm_sim.source_label = Some("rule".into());
                    state.games.tm_sim.scroll_offset = 0;
                    return false;
                }
            };

            let mut numbers: Vec<u64> = Vec::new();
            let mut id: Option<String> = None;
            for token in tokens.iter().skip(idx) {
                if let Some(value) = parse_tm_input_token(token) {
                    numbers.push(value);
                    continue;
                }
                if id.is_none() {
                    id = Some((*token).to_string());
                }
            }

            let Some(input) = numbers.first().copied() else {
                state.status = Some(
                    "Usage: :games tm [run|config] <input> [steps] [strategy_id] | :games tm {rule_code, states, symbols} <input> [steps]"
                        .into(),
                );
                return false;
            };
            let steps_override = numbers.get(1).copied().and_then(|value| {
                if value > u32::MAX as u64 {
                    None
                } else {
                    Some(value as u32)
                }
            });

            if let Some((rule_code, states, symbols)) = rule_tuple {
                if states == 0 || symbols < 2 {
                    let msg: String = if states == 0 {
                        "TM rule tuple: states must be >= 1".into()
                    } else {
                        "TM rule tuple: symbols must be >= 2".into()
                    };
                    state.status = Some(msg.clone());
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.strategy_inspect.open = false;
                    state.games.analysis.open = false;
                    state.games.ca_sim.open = false;
                    state.games.tm_sim.open = true;
                    state.games.tm_sim.last_error = Some(msg);
                    state.games.tm_sim.definition = None;
                    state.games.tm_sim.input = Some(input);
                    state.games.tm_sim.steps_override = steps_override;
                    state.games.tm_sim.source_label = Some("rule".into());
                    state.games.tm_sim.scroll_offset = 0;
                    return false;
                }
                let (transitions, _remaining) = nit_games::strategy::decode_tm_rule_code_wolfram(
                    rule_code,
                    states as usize,
                    symbols as usize,
                );
                let output_map: Vec<nit_games::game::Action> = (0..symbols)
                    .map(|idx| {
                        if idx == 0 {
                            nit_games::game::Action::Cooperate
                        } else {
                            nit_games::game::Action::Defect
                        }
                    })
                    .collect();
                let max_steps = steps_override.unwrap_or(256);
                let def = nit_games::output::StrategyDefinition {
                    id: format!("tm_rule_{rule_code}_{states}x{symbols}"),
                    name: Some(format!("Rule {rule_code} ({states}x{symbols})")),
                    kind: nit_games::config::StrategySpecKind::OneSidedTm {
                        states,
                        symbols,
                        start_state: 1,
                        blank: 0,
                        fallback_symbol: Some(0),
                        max_steps_per_round: max_steps,
                        input_mode: nit_games::strategy::InputMode::OpponentLastAction,
                        output_map,
                        transitions,
                        rule_code: Some(rule_code),
                    },
                    rng_seed_a: None,
                    rng_seed_b: None,
                };

                state.games.run_browser.open = false;
                state.games.replay.open = false;
                state.games.strategy_inspect.open = false;
                state.games.analysis.open = false;
                state.games.ca_sim.open = false;
                state.games.tm_sim.open = true;
                state.games.tm_sim.last_error = None;
                state.games.tm_sim.definition = Some(def);
                state.games.tm_sim.input = Some(input);
                state.games.tm_sim.steps_override = steps_override;
                state.games.tm_sim.source_label = Some("rule".into());
                state.games.tm_sim.scroll_offset = 0;
                state.status = Some("TM simulation opened (rule tuple)".into());
                return false;
            }

            let mut source_label = source.to_string();
            let defs: Vec<nit_games::output::StrategyDefinition>;
            match source {
                "run" => {
                    if let Some(run) = state.games.last_run.as_ref() {
                        defs = run.strategies.clone();
                    } else {
                        state.status = Some("No run loaded for TM simulation".into());
                        return false;
                    }
                }
                _ => {
                    let config_text = state.editor_buffer().content_as_string();
                    match nit_games::config::GamesConfig::from_toml_with_root(
                        &config_text,
                        Some(&state.workspace_root),
                    ) {
                        Ok(config) => {
                            defs = config
                                .strategies
                                .iter()
                                .map(|spec| nit_games::output::StrategyDefinition {
                                    id: spec.id.clone(),
                                    name: spec.name.clone(),
                                    kind: spec.kind.clone(),
                                    rng_seed_a: None,
                                    rng_seed_b: None,
                                })
                                .collect();
                            source_label = "config".into();
                        }
                        Err(err) => {
                            let msg = format!("Config error: {err}");
                            state.status = Some(msg.clone());
                            state.games.run_browser.open = false;
                            state.games.replay.open = false;
                            state.games.strategy_inspect.open = false;
                            state.games.analysis.open = false;
                            state.games.ca_sim.open = false;
                            state.games.tm_sim.open = true;
                            state.games.tm_sim.last_error = Some(msg);
                            state.games.tm_sim.definition = None;
                            state.games.tm_sim.input = Some(input);
                            state.games.tm_sim.steps_override = steps_override;
                            state.games.tm_sim.source_label = Some("config".into());
                            state.games.tm_sim.scroll_offset = 0;
                            return false;
                        }
                    }
                }
            }

            let mut tm_defs: Vec<nit_games::output::StrategyDefinition> = defs
                .into_iter()
                .filter(|def| {
                    matches!(
                        def.kind,
                        nit_games::config::StrategySpecKind::OneSidedTm { .. }
                    )
                })
                .collect();

            let selected = if let Some(id) = id.as_ref() {
                tm_defs
                    .iter()
                    .position(|def| def.id == *id)
                    .map(|idx| tm_defs.remove(idx))
            } else if tm_defs.len() == 1 {
                tm_defs.pop()
            } else {
                None
            };

            let Some(def) = selected else {
                if tm_defs.is_empty() {
                    state.status = Some("No one-sided TM strategies found".into());
                } else {
                    state.status = Some("Multiple TM strategies found; specify an id".into());
                }
                return false;
            };

            state.games.run_browser.open = false;
            state.games.replay.open = false;
            state.games.strategy_inspect.open = false;
            state.games.analysis.open = false;
            state.games.ca_sim.open = false;
            state.games.tm_sim.open = true;
            state.games.tm_sim.last_error = None;
            state.games.tm_sim.definition = Some(def);
            state.games.tm_sim.input = Some(input);
            state.games.tm_sim.steps_override = steps_override;
            state.games.tm_sim.source_label = Some(source_label);
            state.games.tm_sim.scroll_offset = 0;
            state.status = Some("TM simulation opened".into());
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"ca") => {
            let mut idx = 2usize;
            let mut source = "config";
            if let Some(token) = tokens.get(idx) {
                if *token == "run" {
                    source = "run";
                    idx += 1;
                } else if *token == "config" {
                    source = "config";
                    idx += 1;
                }
            }

            let rule_tuple = match parse_ca_rule_tuple(trimmed) {
                Ok(value) => value,
                Err(msg) => {
                    state.status = Some(msg.clone());
                    state.games.run_browser.open = false;
                    state.games.replay.open = false;
                    state.games.strategy_inspect.open = false;
                    state.games.analysis.open = false;
                    state.games.tm_sim.open = false;
                    state.games.ca_sim.open = true;
                    state.games.ca_sim.last_error = Some(msg);
                    state.games.ca_sim.definition = None;
                    state.games.ca_sim.input = None;
                    state.games.ca_sim.steps_override = None;
                    state.games.ca_sim.source_label = Some("rule".into());
                    state.games.ca_sim.scroll_offset = 0;
                    return false;
                }
            };

            let mut numbers: Vec<u64> = Vec::new();
            let mut id: Option<String> = None;
            for token in tokens.iter().skip(idx) {
                if let Some(value) = parse_tm_input_token(token) {
                    numbers.push(value);
                    continue;
                }
                if token.contains('{') || token.contains('}') || token.contains(',') {
                    continue;
                }
                if id.is_none() {
                    id = Some((*token).to_string());
                }
            }

            let Some(input) = numbers.first().copied() else {
                state.status = Some(
                    "Usage: :games ca [run|config] <input> [steps] [strategy_id] | :games ca {n,k,r} <input> [steps]"
                        .into(),
                );
                return false;
            };
            let steps_override = numbers.get(1).copied().and_then(|value| {
                if value > u32::MAX as u64 {
                    None
                } else {
                    Some(value as u32)
                }
            });

            if let Some((n, k, two_r, t)) = rule_tuple {
                let def = nit_games::output::StrategyDefinition {
                    id: format!("ca_rule_{n}_{k}_{two_r}_{t}"),
                    name: Some(format!(
                        "CA rule {n} (k={k}, r={}, t={t})",
                        two_r as f32 / 2.0
                    )),
                    kind: nit_games::config::StrategySpecKind::Ca {
                        n,
                        k,
                        r: two_r as f32 / 2.0,
                        t,
                    },
                    rng_seed_a: None,
                    rng_seed_b: None,
                };

                state.games.run_browser.open = false;
                state.games.replay.open = false;
                state.games.strategy_inspect.open = false;
                state.games.analysis.open = false;
                state.games.tm_sim.open = false;
                state.games.ca_sim.open = true;
                state.games.ca_sim.last_error = None;
                state.games.ca_sim.definition = Some(def);
                state.games.ca_sim.input = Some(input);
                state.games.ca_sim.steps_override = steps_override;
                state.games.ca_sim.source_label = Some("rule".into());
                state.games.ca_sim.scroll_offset = 0;
                state.status = Some("CA simulation opened (rule tuple)".into());
                return false;
            }

            let mut source_label = source.to_string();
            let defs: Vec<nit_games::output::StrategyDefinition>;
            match source {
                "run" => {
                    if let Some(run) = state.games.last_run.as_ref() {
                        defs = run.strategies.clone();
                    } else {
                        state.status = Some("No run loaded for CA simulation".into());
                        return false;
                    }
                }
                _ => {
                    let config_text = state.editor_buffer().content_as_string();
                    match nit_games::config::GamesConfig::from_toml_with_root(
                        &config_text,
                        Some(&state.workspace_root),
                    ) {
                        Ok(config) => {
                            defs = config
                                .strategies
                                .iter()
                                .map(|spec| nit_games::output::StrategyDefinition {
                                    id: spec.id.clone(),
                                    name: spec.name.clone(),
                                    kind: spec.kind.clone(),
                                    rng_seed_a: None,
                                    rng_seed_b: None,
                                })
                                .collect();
                            source_label = "config".into();
                        }
                        Err(err) => {
                            let msg = format!("Config error: {err}");
                            state.status = Some(msg.clone());
                            state.games.run_browser.open = false;
                            state.games.replay.open = false;
                            state.games.strategy_inspect.open = false;
                            state.games.analysis.open = false;
                            state.games.tm_sim.open = false;
                            state.games.ca_sim.open = true;
                            state.games.ca_sim.last_error = Some(msg);
                            state.games.ca_sim.definition = None;
                            state.games.ca_sim.input = Some(input);
                            state.games.ca_sim.steps_override = steps_override;
                            state.games.ca_sim.source_label = Some("config".into());
                            state.games.ca_sim.scroll_offset = 0;
                            return false;
                        }
                    }
                }
            }

            let mut ca_defs: Vec<nit_games::output::StrategyDefinition> = defs
                .into_iter()
                .filter(|def| matches!(def.kind, nit_games::config::StrategySpecKind::Ca { .. }))
                .collect();

            let selected = if let Some(id) = id.as_ref() {
                ca_defs
                    .iter()
                    .position(|def| def.id == *id)
                    .map(|idx| ca_defs.remove(idx))
            } else if ca_defs.len() == 1 {
                ca_defs.pop()
            } else {
                None
            };

            let Some(def) = selected else {
                if ca_defs.is_empty() {
                    state.status = Some("No CA strategies found".into());
                } else {
                    state.status = Some("Multiple CA strategies found; specify an id".into());
                }
                return false;
            };

            state.games.run_browser.open = false;
            state.games.replay.open = false;
            state.games.strategy_inspect.open = false;
            state.games.analysis.open = false;
            state.games.tm_sim.open = false;
            state.games.ca_sim.open = true;
            state.games.ca_sim.last_error = None;
            state.games.ca_sim.definition = Some(def);
            state.games.ca_sim.input = Some(input);
            state.games.ca_sim.steps_override = steps_override;
            state.games.ca_sim.source_label = Some(source_label);
            state.games.ca_sim.scroll_offset = 0;
            state.status = Some("CA simulation opened".into());
            false
        }
        _ if tokens.first() == Some(&"games")
            && matches!(tokens.get(1), Some(&"analyze") | Some(&"analyse")) =>
        {
            let defaults = AnalysisConfig::default();
            let mut tail_rounds = defaults.tail_rounds;
            let mut trajectory_samples = defaults.trajectory_samples;
            let mut path: Option<String> = None;

            for arg in trimmed.split_whitespace().skip(2) {
                if let Some((key, value)) = arg.split_once('=') {
                    match key.to_ascii_lowercase().as_str() {
                        "tail" | "tail_rounds" => {
                            if let Ok(parsed) = value.parse::<usize>() {
                                tail_rounds = parsed;
                            }
                        }
                        "samples" | "trajectory_samples" => {
                            if let Ok(parsed) = value.parse::<usize>() {
                                trajectory_samples = parsed;
                            }
                        }
                        "path" => {
                            if !value.is_empty() {
                                path = Some(normalize_path_token(value));
                            }
                        }
                        _ => {}
                    }
                } else if path.is_none() {
                    path = Some(normalize_path_token(arg));
                }
            }

            if let Some(candidate) = path.as_ref() {
                if candidate.trim().is_empty() {
                    path = None;
                }
            }

            if path.is_none() && state.games.last_history_path.is_none() {
                state.status = Some("No history log available to analyze".into());
            } else {
                state.games.pending_analyze = Some(GamesAnalysisRequest {
                    path,
                    tail_rounds,
                    trajectory_samples,
                });
                state.games.analysis.open = true;
                state.games.analysis.last_error = None;
                state.games.analysis.summary = None;
                state.games.analysis.preview = None;
                state.games.analysis.scroll_offset = 0;
                state.status = Some("Games analysis queued".into());
            }
            false
        }
        ["games", "export"] => {
            state.games.pending_export = true;
            false
        }
        ["gol", "seed"] => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
            false
        }
        ["seed", "view"] if state.app_kind == AppKind::Gol => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
            false
        }
        ["gol", "encoder"] => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
            false
        }
        ["gol", "encoder", name] => {
            if let Some(id) = SeedEncoderId::from_str_name(name) {
                state.visualizer.seed_encoder = id;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Encoder: {}", id.label()));
            } else {
                state.status = Some(format!("Unknown encoder: {name}"));
            }
            false
        }
        ["seed", "encoder"] if state.app_kind == AppKind::Gol => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
            false
        }
        ["seed", "encoder", name] if state.app_kind == AppKind::Gol => {
            if let Some(id) = SeedEncoderId::from_str_name(name) {
                state.visualizer.seed_encoder = id;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Encoder: {}", id.label()));
            } else {
                state.status = Some(format!("Unknown encoder: {name}"));
            }
            false
        }
        _ if tokens.first() == Some(&"gol") && tokens.get(1) == Some(&"rule") => {
            if tokens.len() == 2 {
                log_rule_overview(state);
            } else {
                let selector = trimmed
                    .split_whitespace()
                    .skip(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                match state.rule_catalog.select(&selector) {
                    Ok(selected) => apply_rule_selection(state, selected, true),
                    Err(err) => {
                        state.status = Some(format!(
                            "Invalid GoL rule '{selector}': {err}. Try B3/S23 or 'conway'."
                        ));
                    }
                }
            }
            false
        }
        _ if tokens.first() == Some(&"gol") && tokens.get(1) == Some(&"rules") => {
            log_rule_list(state);
            false
        }
        ["petri", "hide"] | ["hide", "petri"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
            false
        }
        ["petri", "show"] | ["show", "petri"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
            false
        }
        other => {
            state.status = Some(format!("Unknown command: {}", other.join(" ")));
            false
        }
    }
}

fn is_help_command_tokens(tokens: &[&str]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let mut saw_keyword = false;
    let mut saw_question = false;
    for token in tokens {
        match *token {
            "help" | "commands" => saw_keyword = true,
            "?" => saw_question = true,
            "-" | "/" | "|" | "–" | "—" => {}
            _ => return false,
        }
    }
    saw_keyword || saw_question
}

fn apply_rule_selection(state: &mut AppState, selected: SelectedRule, persist: bool) {
    let label = selected.name_first_label();
    match state.set_gol_rule(selected, persist) {
        Ok(changed) => {
            if changed {
                let suffix = if state.visualizer.running {
                    " Restarting Petri Dish session."
                } else {
                    ""
                };
                state.status = Some(format!("GoL rule set to {label}.{suffix}"));
            } else {
                state.status = Some(format!("GoL rule unchanged: {label}."));
            }
        }
        Err(err) => {
            state.status = Some(format!("GoL rule set to {label} (save failed: {err})"));
        }
    }
}

fn normalize_path_token(value: &str) -> String {
    let trimmed = value.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|v| v.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    unquoted.trim().to_string()
}

const MAX_FAMILY_RUN_MACHINES_CPU: usize = 10_000;
const MAX_FAMILY_RUN_MACHINES_METAL: usize = 100_000;
const DEFAULT_FAMILY_CA_T: u32 = 10;
// Notebook default for `TuringMachineStrategy[tm]`.
const DEFAULT_FAMILY_TM_MAX_STEPS: u32 = 1000;
const FAMILY_RUN_CONFIG_PREFIX: &str = "# generated by :games run <family> {...}";
const MAX_FAMILY_TOTAL_MATCHES_CPU: u128 = 500_000;
const MAX_FAMILY_TOTAL_ROUND_OPS_CPU: u128 = 50_000_000;

pub fn apply_family_run_runtime_overrides(config: &mut nit_games::NormalizedConfig) {
    config.engine.mode = nit_games::EngineMode::Batch;
    config.event_log.enabled = false;
    config.history.enabled = false;
    config.engine.fast_eval = true;
}

fn build_family_run_override(
    _state: &AppState,
    family: &str,
    input: &str,
    force: bool,
) -> Result<GamesFamilyRunRequest, String> {
    let family = family.to_ascii_lowercase();
    match family.as_str() {
        "fsm" => {
            parse_fsm_family_tuple(input)?;
        }
        "tm" => {
            parse_tm_family_request(input)?;
        }
        "ca" => {
            parse_ca_family_tuple(input)?;
        }
        _ => {
            return Err(
                "Usage: :games run [force] <fsm|ca|tm> {params} (e.g. :games run fsm {2, 2})"
                    .into(),
            )
        }
    }

    Ok(GamesFamilyRunRequest {
        family,
        input: input.to_string(),
        force,
    })
}

pub fn build_family_run_override_for_request(
    workspace_root: &std::path::Path,
    config_text: &str,
    request: &GamesFamilyRunRequest,
) -> Result<GamesRunOverride, String> {
    build_family_run_override_for_request_with_timings(workspace_root, config_text, request)
        .map(|(override_run, _)| override_run)
}

pub fn build_family_run_override_for_request_with_timings(
    workspace_root: &std::path::Path,
    config_text: &str,
    request: &GamesFamilyRunRequest,
) -> Result<(GamesRunOverride, FamilyRunBuildTimings), String> {
    let base_config = nit_games::config::GamesConfig::family_run_base_from_toml_with_root(
        config_text,
        Some(workspace_root),
    )
    .map_err(|err| format!("Config error: {err}"))?;
    build_family_run_override_from_base_config_with_timings(
        workspace_root,
        config_text,
        request,
        base_config,
    )
}

pub fn build_family_run_override_from_base_config(
    workspace_root: &std::path::Path,
    config_text: &str,
    request: &GamesFamilyRunRequest,
    base_config: nit_games::config::FamilyRunBaseConfig,
) -> Result<GamesRunOverride, String> {
    build_family_run_override_from_base_config_with_timings(
        workspace_root,
        config_text,
        request,
        base_config,
    )
    .map(|(override_run, _)| override_run)
}

pub fn build_family_run_override_from_base_config_with_timings(
    workspace_root: &std::path::Path,
    config_text: &str,
    request: &GamesFamilyRunRequest,
    base_config: nit_games::config::FamilyRunBaseConfig,
) -> Result<(GamesRunOverride, FamilyRunBuildTimings), String> {
    let build_started = Instant::now();
    let config = &base_config;
    let metal_available = config.engine.accelerator.allows_metal();

    let generation_started = Instant::now();
    let (strategies, label) = match request.family.as_str() {
        "fsm" => {
            let (states, actions) = parse_fsm_family_tuple(&request.input)?;
            let (specs, stats) = generate_fsm_family_strategies(
                workspace_root,
                states,
                actions,
                config.engine.fsm_grouping,
            )?;
            (
                specs,
                format!(
                    "fsm {{s={states}, k={actions}, grouping={}}}; raw→canonical→unique = {}→{}→{}",
                    fsm_grouping_mode_label(config.engine.fsm_grouping),
                    stats.raw_count,
                    stats.canonical_count,
                    stats.unique_behavior_count
                ),
            )
        }
        "tm" => {
            let (states, symbols, max_steps_override) = parse_tm_family_request(&request.input)?;
            let (blank, default_max_steps) = default_tm_family_params(config.tm_blank_hint);
            let max_steps = max_steps_override.unwrap_or(default_max_steps);
            let specs =
                generate_tm_family_strategies(states, symbols, blank, max_steps, metal_available)?;
            (
                specs,
                format!("tm {{s={states}, k={symbols}, max_steps={max_steps}}}"),
            )
        }
        "ca" => {
            let (symbols, two_r, t) = parse_ca_family_tuple(&request.input)?;
            let specs = generate_ca_family_strategies(symbols, two_r, t, metal_available)?;
            (
                specs,
                format!("ca {{k={symbols}, r={}, t={t}}}", two_r as f32 / 2.0),
            )
        }
        _ => {
            return Err(
                "Usage: :games run [force] <fsm|ca|tm> {params} (e.g. :games run fsm {2, 2})"
                    .into(),
            )
        }
    };
    let generation_elapsed = generation_started.elapsed();

    if strategies.is_empty() {
        return Err(format!("No strategies generated for {label}"));
    }
    let generated_strategies = strategies.len();
    let estimate_started = Instant::now();
    let total_matches =
        estimate_total_matches(strategies.len(), config.repetitions, config.self_play)?;
    let total_round_ops = total_matches.saturating_mul(config.rounds.max(1) as u128);
    let estimate_elapsed = estimate_started.elapsed();
    let metal_available = config.engine.accelerator.allows_metal();
    if !request.force && !metal_available {
        if total_matches > MAX_FAMILY_TOTAL_MATCHES_CPU {
            return Err(format!(
                "Family run too large for CPU: {total_matches} matches exceed cap {MAX_FAMILY_TOTAL_MATCHES_CPU}. Use `accelerator = \"metal\"` or force."
            ));
        }
        if total_round_ops > MAX_FAMILY_TOTAL_ROUND_OPS_CPU {
            return Err(format!(
                "Family run too large for CPU: {total_round_ops} round-ops exceeds cap {MAX_FAMILY_TOTAL_ROUND_OPS_CPU}. Use `accelerator = \"metal\"` or force."
            ));
        }
    }
    let normalize_started = Instant::now();
    let mut config = base_config.into_normalized(strategies);
    apply_family_run_runtime_overrides(&mut config);
    let normalize_elapsed = normalize_started.elapsed();
    if config.strategies.is_empty() {
        return Err(format!("No strategies generated for {label}"));
    }
    let (tm_filter, tm_filter_elapsed) = if request.family == "tm" {
        let filter_started = Instant::now();
        let (filtered, diagnostics) =
            nit_games::try_select_halting_turing_machine_strategies_with_diagnostics(config)?;
        config = filtered;
        (Some(diagnostics), Some(filter_started.elapsed()))
    } else {
        (None, None)
    };
    if config.strategies.is_empty() {
        return Err(format!("No strategies generated for {label}"));
    }
    let generated_text = format!("{FAMILY_RUN_CONFIG_PREFIX}\n# {label}\n{config_text}");
    Ok((
        GamesRunOverride {
            config,
            config_text: generated_text,
            label,
            family_mode: true,
        },
        FamilyRunBuildTimings {
            generated_strategies,
            generation_elapsed,
            estimate_elapsed,
            normalize_elapsed,
            tm_filter_elapsed,
            tm_filter,
            total_elapsed: build_started.elapsed(),
        },
    ))
}

fn estimate_total_matches(
    strategy_count: usize,
    repetitions: u32,
    self_play: bool,
) -> Result<u128, String> {
    let count = strategy_count as u128;
    let reps = repetitions as u128;
    let per_repetition = if self_play {
        count.checked_mul(count)
    } else {
        count.checked_mul(count.saturating_sub(1))
    }
    .ok_or_else(|| "Family run match-count overflow".to_string())?;
    per_repetition
        .checked_mul(reps)
        .ok_or_else(|| "Family run match-count overflow".to_string())
}

fn parse_braced_parts<'a>(input: &'a str, usage: &str) -> Result<(Vec<&'a str>, &'a str), String> {
    let Some(start) = input.find('{') else {
        return Err(usage.to_string());
    };
    let Some(end_rel) = input[start..].find('}') else {
        return Err(format!("{usage} (missing '}}')"));
    };
    let end = start + end_rel;
    let inner = &input[start + 1..end];
    let tail = input[end + 1..].trim();
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        Err(usage.to_string())
    } else {
        Ok((parts, tail))
    }
}

fn parse_fsm_family_tuple(input: &str) -> Result<(usize, usize), String> {
    let usage = "Usage: :games run fsm {states, k} (example: :games run fsm {2, 2})";
    let (parts, _) = parse_braced_parts(input, usage)?;
    if parts.len() != 2 {
        return Err("FSM family tuple must be {states, k}".into());
    }

    let mut states_raw: Option<u64> = None;
    let mut actions_raw: Option<u64> = None;

    for part in parts {
        let token = part.trim();
        if let Some((key_raw, value_raw)) = token.split_once('=') {
            let key = key_raw.trim().to_ascii_lowercase();
            let value = value_raw.trim().parse::<u64>().map_err(|_| {
                format!("FSM family tuple: '{key}' value must be an integer (example: {{2, 2}})")
            })?;
            match key.as_str() {
                "s" | "state" | "states" => states_raw = Some(value),
                "k" | "actions" => actions_raw = Some(value),
                _ => return Err(
                    "FSM family tuple: unknown key; use 's'/'states' and 'k' (example: {s=2, k=2})"
                        .into(),
                ),
            }
            continue;
        }

        if token.eq_ignore_ascii_case("s")
            || token.eq_ignore_ascii_case("state")
            || token.eq_ignore_ascii_case("states")
            || token.eq_ignore_ascii_case("k")
        {
            return Err(
                "FSM family tuple placeholders need numeric values (example: :games run fsm {2, 2})"
                    .into(),
            );
        }

        let value = token.parse::<u64>().map_err(|_| {
            "FSM family tuple entries must be integers (example: :games run fsm {2, 2})".to_string()
        })?;
        if states_raw.is_none() {
            states_raw = Some(value);
        } else if actions_raw.is_none() {
            actions_raw = Some(value);
        } else {
            return Err("FSM family tuple must be {states, k}".into());
        }
    }

    let states_raw = states_raw
        .ok_or_else(|| "FSM family tuple missing states value (example: {2, 2})".to_string())?;
    let actions_raw = actions_raw
        .ok_or_else(|| "FSM family tuple missing k value (example: {2, 2})".to_string())?;

    if states_raw == 0 || states_raw > usize::MAX as u64 {
        return Err(format!(
            "FSM family tuple: states must be in 1..={}",
            usize::MAX
        ));
    }
    if actions_raw != 2 {
        return Err("FSM family tuple currently requires k=2".into());
    }
    Ok((states_raw as usize, actions_raw as usize))
}

fn parse_tm_family_request(input: &str) -> Result<(u16, u8, Option<u32>), String> {
    let usage = "Usage: :games run tm {states, symbols} [max_steps]";
    let (parts, tail) = parse_braced_parts(input, usage)?;
    if parts.len() != 2 {
        return Err("TM family tuple must be {states, symbols}".into());
    }
    let states_raw = parts[0]
        .parse::<u64>()
        .map_err(|_| "TM family tuple: states must be an integer".to_string())?;
    let symbols_raw = parts[1]
        .parse::<u64>()
        .map_err(|_| "TM family tuple: symbols must be an integer".to_string())?;
    if states_raw == 0 || states_raw > u16::MAX as u64 {
        return Err(format!(
            "TM family tuple: states must be in 1..={}",
            u16::MAX
        ));
    }
    if symbols_raw < 2 || symbols_raw > u8::MAX as u64 {
        return Err(format!(
            "TM family tuple: symbols must be in 2..={}",
            u8::MAX
        ));
    }
    let max_steps = if tail.is_empty() {
        None
    } else {
        let Some(token) = tail.split_whitespace().next() else {
            return Err(usage.into());
        };
        if tail.split_whitespace().count() != 1 {
            return Err("TM family max_steps must be a single integer after the tuple".into());
        }
        let value_token = if let Some((key, value)) = token.split_once('=') {
            match key.trim().to_ascii_lowercase().as_str() {
                "steps" | "max_steps" | "max_steps_per_round" => value.trim(),
                _ => {
                    return Err(
                        "TM family max_steps key must be `steps`, `max_steps`, or `max_steps_per_round`"
                            .into(),
                    )
                }
            }
        } else {
            token
        };
        let steps_raw = value_token
            .parse::<u64>()
            .map_err(|_| "TM family max_steps must be an integer".to_string())?;
        if steps_raw == 0 || steps_raw > u32::MAX as u64 {
            return Err(format!("TM family max_steps must be in 1..={}", u32::MAX));
        }
        Some(steps_raw as u32)
    };
    Ok((states_raw as u16, symbols_raw as u8, max_steps))
}

fn parse_ca_family_tuple(input: &str) -> Result<(u8, u32, u32), String> {
    let usage = "Usage: :games run ca {k, r} (or {k, r, t})";
    let (parts, _) = parse_braced_parts(input, usage)?;
    if parts.len() != 2 && parts.len() != 3 {
        return Err("CA family tuple must be {k, r} or {k, r, t}".into());
    }
    let symbols_raw = parts[0]
        .parse::<u64>()
        .map_err(|_| "CA family tuple: k must be an integer".to_string())?;
    if symbols_raw < 2 || symbols_raw > u8::MAX as u64 {
        return Err(format!("CA family tuple: k must be in 2..={}", u8::MAX));
    }
    let two_r = parse_two_r_token(parts[1])
        .ok_or_else(|| "CA family tuple: r must satisfy r >= 0 and IntegerQ[2r]".to_string())?;
    let t = if parts.len() == 3 {
        let t_raw = parts[2]
            .parse::<u64>()
            .map_err(|_| "CA family tuple: t must be an integer".to_string())?;
        if t_raw == 0 || t_raw > u32::MAX as u64 {
            return Err(format!("CA family tuple: t must be in 1..={}", u32::MAX));
        }
        t_raw as u32
    } else {
        DEFAULT_FAMILY_CA_T
    };
    Ok((symbols_raw as u8, two_r, t))
}

fn checked_machine_count(
    count: u128,
    family: &str,
    metal_available: bool,
) -> Result<usize, String> {
    if count == 0 {
        return Err(format!("{family} family has zero machines"));
    }
    let limit = if metal_available {
        MAX_FAMILY_RUN_MACHINES_METAL
    } else {
        MAX_FAMILY_RUN_MACHINES_CPU
    };
    if count > limit as u128 {
        return Err(format!(
            "{family} family run would generate {count} machines; limit is {limit}{}",
            if !metal_available {
                ". Use `accelerator = \"metal\"` to raise the limit"
            } else {
                ""
            }
        ));
    }
    usize::try_from(count).map_err(|_| format!("{family} family machine count does not fit usize"))
}

fn checked_pow_u128_exp(mut base: u128, mut exp: u128) -> Option<u128> {
    let mut out = 1u128;
    while exp > 0 {
        if exp & 1 == 1 {
            out = out.checked_mul(base)?;
        }
        exp >>= 1;
        if exp > 0 {
            base = base.checked_mul(base)?;
        }
    }
    Some(out)
}

struct FsmFamilyStats {
    raw_count: u128,
    canonical_count: usize,
    unique_behavior_count: usize,
}

const FSM_FAMILY_CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct FsmFamilyCacheEntry {
    schema_version: u32,
    states: usize,
    actions: usize,
    grouping_mode: nit_games::FsmGroupingMode,
    canonical_count: usize,
    representative_indices: Vec<u64>,
}

fn fsm_grouping_mode_label(mode: nit_games::FsmGroupingMode) -> &'static str {
    match mode {
        nit_games::FsmGroupingMode::Wnbm => "wnbm",
        nit_games::FsmGroupingMode::Moorem => "moorem",
    }
}

fn fsm_family_cache_dir(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".nit")
        .join("cache")
        .join("games")
        .join("fsm")
}

fn fsm_family_cache_path(
    workspace_root: &Path,
    states: usize,
    actions: usize,
    grouping_mode: nit_games::FsmGroupingMode,
) -> PathBuf {
    fsm_family_cache_dir(workspace_root).join(format!(
        "s{states}_k{actions}_{}_v{FSM_FAMILY_CACHE_SCHEMA_VERSION}.json",
        fsm_grouping_mode_label(grouping_mode)
    ))
}

fn load_fsm_family_cache_entry(
    workspace_root: &Path,
    states: usize,
    actions: usize,
    grouping_mode: nit_games::FsmGroupingMode,
) -> Option<FsmFamilyCacheEntry> {
    let path = fsm_family_cache_path(workspace_root, states, actions, grouping_mode);
    let contents = fs::read(path).ok()?;
    let entry: FsmFamilyCacheEntry = serde_json::from_slice(&contents).ok()?;
    if entry.schema_version != FSM_FAMILY_CACHE_SCHEMA_VERSION
        || entry.states != states
        || entry.actions != actions
        || entry.grouping_mode != grouping_mode
    {
        return None;
    }
    Some(entry)
}

fn persist_fsm_family_cache_entry(workspace_root: &Path, entry: &FsmFamilyCacheEntry) {
    let path = fsm_family_cache_path(
        workspace_root,
        entry.states,
        entry.actions,
        entry.grouping_mode,
    );
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(encoded) = serde_json::to_vec(entry) else {
        return;
    };
    let _ = fs::write(path, encoded);
}

fn generate_fsm_family_strategies(
    workspace_root: &Path,
    states: usize,
    actions: usize,
    grouping_mode: nit_games::FsmGroupingMode,
) -> Result<(Vec<nit_games::StrategySpec>, FsmFamilyStats), String> {
    let total_count = nit_games::fsm_count(states, actions)
        .ok_or_else(|| "FSM family count overflow".to_string())?;
    let cache_entry = load_fsm_family_cache_entry(workspace_root, states, actions, grouping_mode);
    let (representative_indices, canonical_count) = if let Some(entry) = cache_entry {
        (entry.representative_indices, entry.canonical_count)
    } else {
        let representative_indices = nit_games::unique_fsm_behavior_representatives_with_mode(
            states,
            actions,
            grouping_mode,
        )?;
        let canonical_count = nit_games::canonical_fsm_indices(states, actions)?.len();
        persist_fsm_family_cache_entry(
            workspace_root,
            &FsmFamilyCacheEntry {
                schema_version: FSM_FAMILY_CACHE_SCHEMA_VERSION,
                states,
                actions,
                grouping_mode,
                canonical_count,
                representative_indices: representative_indices.clone(),
            },
        );
        (representative_indices, canonical_count)
    };
    let unique_behavior_count = representative_indices.len();
    let mut specs = Vec::with_capacity(unique_behavior_count);
    for rule_index in representative_indices {
        let (outputs, transitions) =
            nit_games::decode_fsm_notebook_index(rule_index, states, actions)
                .map_err(|err| format!("FSM index decode error for {rule_index}: {err}"))?;
        specs.push(nit_games::StrategySpec {
            id: format!("fsm_{rule_index}"),
            name: None,
            kind: nit_games::config::StrategySpecKind::Fsm {
                num_states: states,
                start_state: 0,
                outputs,
                input_mode: Some(nit_games::InputMode::OpponentLastAction),
                transitions,
                index: Some(rule_index),
            },
        });
    }
    Ok((
        specs,
        FsmFamilyStats {
            raw_count: total_count,
            canonical_count,
            unique_behavior_count,
        },
    ))
}

fn default_tm_family_params(blank_hint: Option<u8>) -> (u8, u32) {
    // For compatibility with Results-06 / Code-02 notebook defaults, prefer the
    // notebook's TM max-step default rather than inheriting from the current config.
    let blank = blank_hint.unwrap_or(0);
    (blank, DEFAULT_FAMILY_TM_MAX_STEPS)
}

fn generate_tm_family_strategies(
    states: u16,
    symbols: u8,
    blank: u8,
    max_steps: u32,
    metal_available: bool,
) -> Result<Vec<nit_games::StrategySpec>, String> {
    if blank >= symbols {
        return Err(format!(
            "TM family tuple: blank symbol {blank} out of range for symbols={symbols}"
        ));
    }
    let max_index = nit_games::tm_max_index(states as usize, symbols as usize)
        .ok_or_else(|| "TM family index space overflow".to_string())?;
    let count = max_index
        .checked_add(1)
        .ok_or_else(|| "TM family index space overflow".to_string())?;
    let machine_count = checked_machine_count(count, "TM", metal_available)?;
    let mut specs = Vec::with_capacity(machine_count);
    let output_map: Vec<nit_games::Action> = (0..symbols)
        .map(|symbol| {
            if symbol == 0 {
                nit_games::Action::Cooperate
            } else {
                nit_games::Action::Defect
            }
        })
        .collect();
    for index in 0..machine_count {
        let rule_code = index as u64;
        let (transitions, _remaining) =
            nit_games::decode_tm_rule_code_wolfram(rule_code, states as usize, symbols as usize);
        specs.push(nit_games::StrategySpec {
            id: format!("tm_{rule_code}"),
            name: None,
            kind: nit_games::config::StrategySpecKind::OneSidedTm {
                states,
                symbols,
                start_state: 1,
                blank,
                fallback_symbol: Some(blank),
                max_steps_per_round: max_steps,
                input_mode: nit_games::InputMode::OpponentLastAction,
                output_map: output_map.clone(),
                transitions,
                rule_code: Some(rule_code),
            },
        });
    }
    Ok(specs)
}

fn generate_ca_family_strategies(
    symbols: u8,
    two_r: u32,
    t: u32,
    metal_available: bool,
) -> Result<Vec<nit_games::StrategySpec>, String> {
    let neighborhood = two_r.saturating_add(1) as u128;
    let table_len = checked_pow_u128_exp(symbols as u128, neighborhood)
        .ok_or_else(|| "CA family rule-table size overflow".to_string())?;
    let rule_count = checked_pow_u128_exp(symbols as u128, table_len)
        .ok_or_else(|| "CA family rule count overflow".to_string())?;
    let machine_count = checked_machine_count(rule_count, "CA", metal_available)?;
    let mut specs = Vec::with_capacity(machine_count);
    for index in 0..machine_count {
        let n = index as u64;
        specs.push(nit_games::StrategySpec {
            id: format!("ca_{n}"),
            name: None,
            kind: nit_games::config::StrategySpecKind::Ca {
                n,
                k: symbols,
                r: two_r as f32 / 2.0,
                t,
            },
        });
    }
    Ok(specs)
}

fn parse_ca_rule_tuple(input: &str) -> Result<Option<(u64, u8, u32, u32)>, String> {
    const DEFAULT_CA_T: u32 = 10;
    let Some(start) = input.find('{') else {
        return Ok(None);
    };
    let Some(end_rel) = input[start..].find('}') else {
        return Err("CA rule tuple missing '}'".into());
    };
    let end = start + end_rel;
    let inner = &input[start + 1..end];
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err("CA rule tuple must be {n, k, r} (or {n, k, r, t})".into());
    }
    let n = parts[0]
        .parse::<u64>()
        .map_err(|_| "CA rule tuple: n must be an integer".to_string())?;
    let k_raw = parts[1]
        .parse::<u64>()
        .map_err(|_| "CA rule tuple: k must be an integer".to_string())?;
    if k_raw < 2 || k_raw > u8::MAX as u64 {
        return Err(format!("CA rule tuple: k must be in 2..={}", u8::MAX));
    }
    let two_r = parse_two_r_token(parts[2])
        .ok_or_else(|| "CA rule tuple: r must satisfy r >= 0 and IntegerQ[2r]".to_string())?;
    let t = if parts.len() == 4 {
        let t_raw = parts[3]
            .parse::<u64>()
            .map_err(|_| "CA rule tuple: t must be an integer".to_string())?;
        if t_raw == 0 || t_raw > u32::MAX as u64 {
            return Err(format!("CA rule tuple: t must be in 1..={}", u32::MAX));
        }
        t_raw as u32
    } else {
        DEFAULT_CA_T
    };
    Ok(Some((n, k_raw as u8, two_r, t)))
}

fn parse_two_r_token(token: &str) -> Option<u32> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if let Some((numer, denom)) = token.split_once('/') {
        let numer = numer.trim().parse::<i64>().ok()?;
        let denom = denom.trim().parse::<i64>().ok()?;
        if denom == 0 || numer < 0 {
            return None;
        }
        let two_numer = numer.checked_mul(2)?;
        if two_numer % denom != 0 {
            return None;
        }
        let two_r = two_numer / denom;
        if two_r < 0 || two_r > u32::MAX as i64 {
            return None;
        }
        return Some(two_r as u32);
    }

    let value = token.parse::<f64>().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let doubled = value * 2.0;
    let rounded = doubled.round();
    if (doubled - rounded).abs() > 1e-6 {
        return None;
    }
    if rounded < 0.0 || rounded > u32::MAX as f64 {
        return None;
    }
    Some(rounded as u32)
}

fn parse_tm_rule_tuple(input: &str) -> Result<Option<(u64, u16, u8)>, String> {
    let Some(start) = input.find('{') else {
        return Ok(None);
    };
    let Some(end_rel) = input[start..].find('}') else {
        return Err("TM rule tuple missing '}'".into());
    };
    let end = start + end_rel;
    let inner = &input[start + 1..end];
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() != 3 {
        return Err("TM rule tuple must be {rule_code, states, symbols}".into());
    }
    let rule_code = parts[0]
        .parse::<u64>()
        .map_err(|_| "TM rule tuple: rule_code must be an integer".to_string())?;
    let states_raw = parts[1]
        .parse::<u64>()
        .map_err(|_| "TM rule tuple: states must be an integer".to_string())?;
    let symbols_raw = parts[2]
        .parse::<u64>()
        .map_err(|_| "TM rule tuple: symbols must be an integer".to_string())?;
    if states_raw == 0 || states_raw > u16::MAX as u64 {
        return Err(format!("TM rule tuple: states must be in 1..={}", u16::MAX));
    }
    if symbols_raw == 0 || symbols_raw > u8::MAX as u64 {
        return Err(format!("TM rule tuple: symbols must be in 1..={}", u8::MAX));
    }
    Ok(Some((rule_code, states_raw as u16, symbols_raw as u8)))
}

fn parse_tm_input_token(token: &str) -> Option<u64> {
    if let Ok(value) = token.parse::<u64>() {
        return Some(value);
    }
    let (base_str, exp_str) = token.split_once('^')?;
    if base_str.is_empty() || exp_str.is_empty() {
        return None;
    }
    let base = base_str.parse::<u64>().ok()?;
    let exp = exp_str.parse::<u32>().ok()?;
    if base < 2 {
        return None;
    }
    base.checked_pow(exp)
}

fn strategy_id_prefers_fsm(state: &AppState, id: &str) -> bool {
    if let Some(run) = state.games.last_run.as_ref() {
        if let Some(def) = run.strategies.iter().find(|def| def.id == id) {
            return matches!(def.kind, nit_games::config::StrategySpecKind::Fsm { .. });
        }
    }

    let config_text = state.editor_buffer().content_as_string();
    if let Ok(config) = nit_games::config::GamesConfig::from_toml_with_root(
        &config_text,
        Some(&state.workspace_root),
    ) {
        if let Some(spec) = config.strategies.iter().find(|spec| spec.id == id) {
            return matches!(spec.kind, nit_games::config::StrategySpecKind::Fsm { .. });
        }
    }

    id.eq_ignore_ascii_case("fsm") || id.starts_with("fsm")
}

fn lab_from_tokens(tokens: &[&str]) -> Option<AppKind> {
    tokens
        .first()
        .and_then(|token| lab_from_token(token))
        .or_else(|| tokens.get(1).and_then(|token| lab_from_token(token)))
}

fn lab_from_token(token: &str) -> Option<AppKind> {
    match token {
        "gol" | "life" => Some(AppKind::Gol),
        "games" => Some(AppKind::Games),
        _ => None,
    }
}

fn apply_protocol_selection(state: &mut AppState, mut mode: RuleMode, label: Option<String>) {
    mode.reset();
    state.visualizer.rule_mode = mode;
    state.visualizer.protocol_name = label;
    let rule_ref = state.visualizer.rule_mode.current_rule().clone();
    state.visualizer.rule = rule_ref.rule.to_string();
    let mut selected = SelectedRule::from_rule(rule_ref.rule);
    if let Some(named) = state.rule_catalog.find_by_rule(rule_ref.rule) {
        selected.id = Some(named.id.clone());
        selected.name = Some(named.name.clone());
    } else {
        selected.id = rule_ref.id;
        selected.name = rule_ref.name;
    }
    state.gol_rule_selected = selected;
    state.visualizer.pending_rule_change = true;
}

fn log_rule_overview(state: &mut AppState) {
    state.receive_log(format!(
        "Current GoL rule: {}",
        state.gol_rule_selected.label()
    ));
    let builtins: Vec<String> = state
        .rule_catalog
        .builtins()
        .map(|rule| format!("  {} — {} ({})", rule.id, rule.name, rule.rule))
        .collect();
    if !builtins.is_empty() {
        state.receive_log("Built-in rules:".to_string());
        for line in builtins {
            state.receive_log(line);
        }
    }
}

fn log_rule_list(state: &mut AppState) {
    state.receive_log(format!("GoL rules ({} total):", state.rule_catalog.len()));
    let lines: Vec<String> = state
        .rule_catalog
        .iter()
        .map(|rule| format!("  {} — {} ({})", rule.id, rule.name, rule.rule))
        .collect();
    for line in lines {
        state.receive_log(line);
    }
    state.rule_picker.open = true;
    state.rule_picker.query.clear();
    state.rule_picker.selected = state
        .rule_catalog
        .index_of_selected(&state.gol_rule_selected)
        .unwrap_or(0);
}

fn cycle_seed_overlays(state: &mut VisualizerState) {
    const PRESETS: &[(bool, bool, bool, bool, bool)] = &[
        (false, false, false, false, false),
        (false, false, true, false, false),
        (false, false, true, true, false),
        (false, true, true, true, false),
        (false, true, true, true, true),
    ];
    let current = (
        state.seed_show_grid,
        state.seed_show_bbox,
        state.seed_show_halo,
        state.seed_show_components,
        state.seed_show_inset,
    );
    let idx = PRESETS
        .iter()
        .position(|preset| *preset == current)
        .unwrap_or(0);
    let next = PRESETS[(idx + 1) % PRESETS.len()];
    state.seed_show_grid = next.0;
    state.seed_show_bbox = next.1;
    state.seed_show_halo = next.2;
    state.seed_show_components = next.3;
    state.seed_show_inset = next.4;
}

fn seed_overlay_label(state: &VisualizerState) -> String {
    let mut parts = Vec::new();
    if state.seed_show_halo {
        parts.push("HALO");
    }
    if state.seed_show_components {
        parts.push("COMP");
    }
    if state.seed_show_bbox {
        parts.push("BBOX");
    }
    if state.seed_show_inset {
        parts.push("INSET");
    }
    if parts.is_empty() {
        "OFF".into()
    } else {
        parts.join("+")
    }
}

fn move_inspector(state: &mut AppState, dx: isize, dy: isize) {
    let w = state.visualizer.seed_stats.base_width;
    let h = state.visualizer.seed_stats.base_height;
    if w == 0 || h == 0 {
        return;
    }
    let (x, y) = match state.visualizer.seed_encoder {
        SeedEncoderId::AsciiBytes
        | SeedEncoderId::TokenSpectrum
        | SeedEncoderId::AstStructure
        | SeedEncoderId::ComplexityField => (
            &mut state.visualizer.inspect_ascii_x,
            &mut state.visualizer.inspect_ascii_y,
        ),
        SeedEncoderId::Lifehash16 => (
            &mut state.visualizer.inspect_lifehash_x,
            &mut state.visualizer.inspect_lifehash_y,
        ),
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => (
            &mut state.visualizer.inspect_hilbert_x,
            &mut state.visualizer.inspect_hilbert_y,
        ),
    };
    let nx = clamp_signed(*x as isize + dx, 0, (w - 1) as isize) as usize;
    let ny = clamp_signed(*y as isize + dy, 0, (h - 1) as isize) as usize;
    *x = nx;
    *y = ny;
}

fn inspector_dims(state: &AppState) -> (usize, usize) {
    (
        state.visualizer.seed_stats.base_width,
        state.visualizer.seed_stats.base_height,
    )
}

fn set_inspector_pos(state: &mut AppState, x: usize, y: usize) {
    match state.visualizer.seed_encoder {
        SeedEncoderId::AsciiBytes
        | SeedEncoderId::TokenSpectrum
        | SeedEncoderId::AstStructure
        | SeedEncoderId::ComplexityField => {
            state.visualizer.inspect_ascii_x = x;
            state.visualizer.inspect_ascii_y = y;
        }
        SeedEncoderId::Lifehash16 => {
            state.visualizer.inspect_lifehash_x = x;
            state.visualizer.inspect_lifehash_y = y;
        }
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => {
            state.visualizer.inspect_hilbert_x = x;
            state.visualizer.inspect_hilbert_y = y;
        }
    }
}

fn jump_inspector_to_index(state: &mut AppState, idx: u64) {
    let (w, h) = inspector_dims(state);
    let total = w.saturating_mul(h).max(1) as u64;
    let clamped = idx.min(total.saturating_sub(1));
    match state.visualizer.seed_encoder {
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => {
            let order = hilbert_order_for(w);
            let (x, y) = hilbert_index_to_xy(order, clamped as u32);
            set_inspector_pos(state, x as usize, y as usize);
        }
        _ => {
            let x = (clamped as usize) % w;
            let y = (clamped as usize) / w;
            set_inspector_pos(state, x, y);
        }
    }
}

fn hilbert_order_for(size: usize) -> u32 {
    let mut order = 0u32;
    let mut n = 1usize;
    while n < size {
        n <<= 1;
        order += 1;
    }
    order
}

fn hilbert_index_to_xy(order: u32, index: u32) -> (u32, u32) {
    let mut x = 0u32;
    let mut y = 0u32;
    let mut t = index;
    let mut s = 1u32;
    let n = 1u32 << order;
    while s < n {
        let rx = (t / 2) & 1;
        let ry = (t ^ rx) & 1;
        let (nx, ny) = rot(s, x, y, rx, ry);
        x = nx + s * rx;
        y = ny + s * ry;
        t /= 4;
        s *= 2;
    }
    (x, y)
}

fn rot(n: u32, x: u32, y: u32, rx: u32, ry: u32) -> (u32, u32) {
    if ry == 0 {
        if rx == 1 {
            return (n - 1 - x, n - 1 - y);
        }
        return (y, x);
    }
    (x, y)
}

fn clamp_signed(value: isize, min: isize, max: isize) -> isize {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

#[cfg(test)]
#[path = "tests/state.rs"]
mod tests;
