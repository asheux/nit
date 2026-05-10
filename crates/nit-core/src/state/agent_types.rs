use super::*;

/// Top-level dock tab in the Agent OPS surface. `Patch` is legacy/hidden —
/// `next()` / `prev()` skip past it so the visible tab strip matches the
/// renderer.
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

/// One row in the global archive browser. `search_hay` and `search_tokens`
/// are pre-computed at index build time so per-keystroke fuzzy and BM25
/// scoring stays O(rows) without re-tokenising the full content.
#[derive(Clone, Debug)]
pub struct GlobalArchiveEntry {
    /// `"PROMPT"` | `"REPLY"` | `"PATCH"` | `"EVIDENCE"`.
    pub kind: &'static str,
    pub owner: String,
    /// Truncated display preview (~120 chars).
    pub preview: String,
    /// Mission title or `"ad-hoc: {agent_id}"`.
    pub source: String,
    /// `mission_id` or `agent_id`, matching `source_kind`.
    pub source_id: String,
    pub source_kind: GlobalArchiveSourceKind,
    /// Relative label such as `"saved 2h ago"`.
    pub time_label: String,
    pub archive_micros: Option<u128>,
    /// Path to the `run.json` containing this artifact.
    pub run_path: String,
    /// Position inside the run's `messages` / `patches` / `evidence` array.
    pub artifact_index: usize,
    /// Lowercased haystack for fuzzy matching (kind + owner + source + full_text).
    pub search_hay: String,
    /// Lowercased word tokens for BM25 scoring.
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
    #[serde(default)]
    pub shadow: bool,
}

impl AgentLane {
    /// `kind` is the source of truth, but older snapshots arrive with
    /// `kind == Unknown` and only the lane string filled in (the runner
    /// sets it from the CLI), so fall back to a case-insensitive match
    /// on `lane` to recognise pre-tagged rosters.
    fn lane_matches(&self, kind: AgentLaneKind, name: &str) -> bool {
        self.kind == kind
            || (self.kind == AgentLaneKind::Unknown && self.lane.eq_ignore_ascii_case(name))
    }

    pub fn is_codex(&self) -> bool {
        self.lane_matches(AgentLaneKind::Codex, "codex")
    }

    pub fn is_claude(&self) -> bool {
        self.lane_matches(AgentLaneKind::Claude, "claude")
    }

    /// True for lanes whose backend can accept priority hints from the
    /// `parallel`/`bulk` swarm planner. Excludes lanes whose role
    /// equals their lane name (i.e. unspecialised default lanes).
    pub fn supports_swarm_priority(&self) -> bool {
        let backend_ok = self.lane_matches(AgentLaneKind::Codex, "codex")
            || self.lane_matches(AgentLaneKind::Claude, "claude")
            || self.lane_matches(AgentLaneKind::Gemini, "gemini");
        backend_ok && !self.role.eq_ignore_ascii_case(&self.lane)
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
    /// Index of the user prompt this reply is responding to. `None` for
    /// user prompts themselves and for replies where the prompt is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_msg_idx: Option<usize>,
    /// Optional kind tag, e.g. `"synth"` for synthesis reports.
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

/// Cached rendered rows for the agent console. Keyed by viewport width and
/// message-list epoch so a no-op render can reuse `rows` instead of replaying
/// the wrapping/highlighting pipeline. `breather_slots` records where inline
/// "agent thinking…" breathers must be spliced in for prompts whose replies
/// are still pending.
#[derive(Clone, Debug, Default)]
pub struct AgentConsoleRowsCache {
    pub key: Option<AgentConsoleRowsCacheKey>,
    pub rows: Vec<AgentConsoleRow>,
    pub last_message_was_user: bool,
    /// Positions `(row_index, prompt_msg_idx)` where a breather row should
    /// be inserted before render.
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

/// In-flight authoritative genome evaluation batch for a single agent's
/// TurnCompleted. Holds the worker thread count, the worst delta seen so
/// far, and the mission the turn belonged to. One batch per agent keeps
/// parallel swarm turns from clobbering each other.
#[derive(Clone, Debug, Default)]
pub struct GenomeEvalBatch {
    pub pending: usize,
    pub worst_delta: i32,
    pub mission_id: Option<String>,
}

/// Codex turn queued behind an in-flight Codex turn for the same agent.
/// `prompt_msg_idx` is the index of the user message that triggered it,
/// preserved so the eventual reply can be linked back in the chat log.
#[derive(Clone, Debug)]
pub struct QueuedCodexTurn {
    pub agent_id: String,
    pub mission_id: Option<String>,
    pub prompt: String,
    pub prompt_msg_idx: Option<usize>,
}

/// Claude analogue of [`QueuedCodexTurn`] — see that struct for semantics.
#[derive(Clone, Debug)]
pub struct QueuedClaudeTurn {
    pub agent_id: String,
    pub mission_id: Option<String>,
    pub prompt: String,
    pub prompt_msg_idx: Option<usize>,
}

/// Operator chat-state stashed while an intake turn is in flight. The
/// bus-event handler resumes the deferred dispatch using this snapshot
/// once the intake decision lands (or on parse-fail / timeout / abort).
/// Runtime-only — `started_at` is an `Instant`, so this is never persisted.
#[derive(Clone, Debug)]
pub struct PendingIntake {
    pub mission_id: Option<String>,
    pub prompt_msg_idx: usize,
    pub channel: AgentChannel,
    pub force_new: bool,
    pub raw_prompt: String,
    pub target_cwd: PathBuf,
    pub target_agent_id: String,
    pub intake_agent_id: String,
    pub started_at: std::time::Instant,
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
