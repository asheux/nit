//! Core state, agent bus protocol, configuration, and text buffers for the nit workspace.

#![forbid(unsafe_code)]

pub mod actions;
pub mod agent_bus;
pub mod buffer;
pub mod config;
pub mod cursor;
pub mod genome_report;
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
pub use buffer::{BufferEdit, BufferPoint, LineDiffStatus};
pub use config::{
    AgentGenomeConfig, EditorConfig, GenomeGateConfig, GolConfig, GolRuleConfig, GolRulesConfig,
    GolSearchConfig, GolSearchIntensity, GolSeedSource, GolSnapshotsConfig, GolUserRule,
    HighlightConfig, HighlightEngine, Settings, SnapshotPrunePolicy,
};
pub use cursor::Cursor;
pub use genome_report::{
    compute_genome_diff, compute_genome_report, format_genome_diff, format_genome_report,
    EncoderDiff, EncoderScore, GenomeDiff, GenomeRecommendation, GenomeReport, GenomeTier,
    GrowthClass, ParsimonyInfo, RecommendationSeverity, GENOME_AGENT_INSTRUCTIONS,
};
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
    apply_action, apply_family_run_runtime_overrides, build_family_run_override_for_request,
    build_family_run_override_for_request_with_timings, build_family_run_override_from_base_config,
    build_family_run_override_from_base_config_with_timings, AgentAlert, AgentAlertSeverity,
    AgentChannel, AgentConsoleRow, AgentConsoleRowKind, AgentConsoleRowsCache,
    AgentConsoleRowsCacheKey, AgentConsoleTab, AgentDiagnosticEvent, AgentLane, AgentLaneKind,
    AgentMessage, AgentOpsTab, AgentStatus, AgentsState, AppState, DirEntryModel, EvidenceItem,
    FamilyRunBuildTimings, FileTreeKind, FileTreeRow, FileTreeState, GamesAnalysisRequest,
    GamesAnalysisState, GamesCaSimState, GamesConfigPreview, GamesFamilyRunRequest,
    GamesReplayRequest, GamesReplayState, GamesRunBrowserState, GamesRunEntry, GamesRunOverride,
    GamesState, GamesStatus, GamesStrategyInspectState, GateMonitorSubView, GenomeShadowEval,
    GlobalArchiveEntry, GlobalArchiveSourceKind, GolRenderMode, JobState, LogBuffer,
    McpConnectionState, McpStatus, Metrics, MissionPhase, MissionRecord, PatchProposal,
    PatchStatus, QueuedClaudeTurn, QueuedCodexTurn, RosterTreeBranch, RosterTreeSelection,
    SavedRunHistoryFilter, SavedRunHistoryPendingAction, SyntaxDebugInfo, UiSelection,
    UiSelectionPane, VisualizerMode, VisualizerRuleEntry, VisualizerState, YankKind,
    CONSOLE_SCROLL_BOTTOM,
};
pub use viewport::Viewport;
