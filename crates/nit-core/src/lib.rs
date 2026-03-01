#![forbid(unsafe_code)]

pub mod actions;
pub mod agent_bus;
pub mod buffer;
pub mod config;
pub mod cursor;
pub mod gol_rules;
pub mod io;
pub mod lab;
pub mod mode;
pub mod pane;
pub mod prompt;
pub mod rule_config;
pub mod rule_protocol;
pub mod search;
pub mod seed;
pub mod state;
pub mod viewport;

pub use actions::Action;
pub use agent_bus::{AgentBusEvent, AgentTokenCount};
pub use buffer::Buffer;
pub use buffer::{BufferEdit, BufferPoint};
pub use config::{
    EditorConfig, GolConfig, GolRuleConfig, GolRulesConfig, GolSearchConfig, GolSearchIntensity,
    GolSeedSource, GolSnapshotsConfig, GolUserRule, HighlightConfig, HighlightEngine, Settings,
    SnapshotPrunePolicy,
};
pub use cursor::Cursor;
pub use gol_rules::{load_rule_catalog, NamedRule, RuleCatalog, RuleSelectError, SelectedRule};
pub use io::{load_to_string, save_buffer};
pub use lab::{AppKind, LabId, LabSpec};
pub use mode::Mode;
pub use pane::PaneId;
pub use prompt::Prompt;
pub use rule_config::{load_rule_config, persist_rule_selection, RuleConfigLoad, RulePersistence};
pub use rule_protocol::{
    builtin_protocols, parse_protocol_spec, ProtocolPreset, RuleMode, RulePhase, RuleProtocol,
    RuleRef,
};
pub use search::{FuzzySearchState, SearchMode, SearchResultFile, SearchResultMatch};
pub use seed::{
    encode_seed, EncodedSeed, SeedEncoderId, SeedParams, SeedPlacement, SeedPreviewMode, SeedStats,
    SeedSymmetry,
};
pub use state::{
    apply_action, build_family_run_override_for_request, AgentAlert, AgentAlertSeverity,
    AgentChannel, AgentConsoleRow, AgentConsoleRowKind, AgentConsoleRowsCache,
    AgentConsoleRowsCacheKey, AgentConsoleTab, AgentDiagnosticEvent, AgentLane, AgentLaneKind,
    AgentMessage, AgentOpsTab, AgentStatus, AgentsState, AppState, DirEntryModel, EvidenceItem,
    FileTreeKind, FileTreeRow, FileTreeState, GamesAnalysisRequest, GamesAnalysisState,
    GamesCaSimState, GamesConfigPreview, GamesFamilyRunRequest, GamesReplayRequest,
    GamesReplayState, GamesRunBrowserState, GamesRunEntry, GamesRunOverride, GamesState,
    GamesStatus, GamesStrategyInspectState, GolRenderMode, JobState, LogBuffer, McpConnectionState,
    McpStatus, Metrics, MissionPhase, MissionRecord, PatchProposal, PatchStatus, QueuedCodexTurn,
    SyntaxDebugInfo, UiSelection, UiSelectionPane, VisualizerMode, VisualizerRuleEntry,
    VisualizerState, YankKind,
};
pub use viewport::Viewport;
