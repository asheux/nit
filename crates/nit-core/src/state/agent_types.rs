use super::*;

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
    #[serde(default)]
    pub shadow: bool,
}

impl AgentLane {
    /// Lane kinds may arrive untagged from older snapshots. The kind enum
    /// is the source of truth, but when it is Unknown we fall back to the
    /// case-insensitive lane string (which the runner sets from the CLI).
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

    pub fn supports_swarm_priority(&self) -> bool {
        let backend_supports_priority = self.lane_matches(AgentLaneKind::Codex, "codex")
            || self.lane_matches(AgentLaneKind::Claude, "claude")
            || self.lane_matches(AgentLaneKind::Gemini, "gemini");
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

/// Operator chat-state stashed while an intake turn is in flight. The
/// bus-event handler resumes the deferred dispatch using this snapshot
/// once the intake decision lands (or on parse-fail / timeout / abort).
/// Runtime-only: never serialized — `started_at` is an `Instant`.
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
