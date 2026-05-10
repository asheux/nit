use crate::{
    actions::Action,
    buffer::Buffer,
    config::{GolSeedSource, Settings},
    gol_rules::{RuleCatalog, SelectedRule},
    lab::AppKind,
    mode::Mode,
    pane::PaneId,
    prompt::Prompt,
    rule_protocol::{RuleMode, RuleRef},
    search::{FuzzySearchState, SearchMode},
    seed::{SeedEncoderId, SeedParams, SeedPreviewMode, SeedStats, SeedViewMode},
    viewport::Viewport,
};
use nit_gol::{AttractorEvent, AutoStopPolicy, Rule};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod action_apply;
mod agent_types;
mod agents_state;
mod cmd_line;
mod defaults;
mod family_run;
pub mod file_tree;
mod games;
pub mod multipane;
mod pickers;
mod text_input;
mod visualizer;
pub use action_apply::apply_action;
pub use agent_types::{
    AgentAlert, AgentAlertSeverity, AgentChannel, AgentConsoleRow, AgentConsoleRowKind,
    AgentConsoleRowsCache, AgentConsoleRowsCacheKey, AgentConsoleTab, AgentDiagnosticEvent,
    AgentLane, AgentLaneKind, AgentMessage, AgentOpsTab, AgentStatus, AgentTurnState, EvidenceItem,
    GenomeEvalBatch, GenomeShadowEval, GlobalArchiveEntry, GlobalArchiveSourceKind,
    McpConnectionState, McpStatus, MissionPhase, MissionRecord, PatchProposal, PatchStatus,
    PendingIntake, QueuedClaudeTurn, QueuedCodexTurn, RosterTreeBranch, RosterTreeSelection,
    SavedRunHistoryFilter, SavedRunHistoryPendingAction,
};
pub use agents_state::AgentsState;
pub use family_run::{
    apply_family_run_runtime_overrides, build_family_run_override_for_request,
    build_family_run_override_for_request_with_timings, build_family_run_override_from_base_config,
    build_family_run_override_from_base_config_with_timings,
};
pub use file_tree::{DirEntryModel, FileTreeKind, FileTreeRow, FileTreeState};
pub use games::{
    FamilyRunBuildTimings, GamesAnalysisRequest, GamesAnalysisState, GamesCaSimState,
    GamesConfigPreview, GamesFamilyRunRequest, GamesMatchHistoryState, GamesReplayRequest,
    GamesReplayState, GamesRunBrowserState, GamesRunEntry, GamesRunOverride, GamesState,
    GamesStatus, GamesStrategyInspectState, GamesTmSimState,
};
pub use multipane::{DirSearchState, MultipaneState, PaneSelection, PaneSession};
pub use pickers::{ProtocolPickerState, RulePickerState};
pub use text_input::{CommandLine, EditorSearch, SearchPrompt};
pub use visualizer::{GolRenderMode, VisualizerMode, VisualizerRuleEntry, VisualizerState};

use cmd_line::{apply_protocol_selection, apply_rule_selection, handle_command_line};
use family_run::build_family_run_override;
use games::open_games_history_popup;
use visualizer::{
    cycle_seed_overlays, inspector_dims, jump_inspector_to_index, move_inspector,
    seed_overlay_label, set_inspector_pos,
};

use defaults::{
    artifacts_popup_last_max_scroll_default, chat_input_scroll_default,
    claude_max_parallel_turns_default, codex_max_parallel_turns_default,
    gate_monitor_max_scroll_default, scroll_cache_sentinel, swarm_default_mission_default,
    swarm_default_template_default,
};
#[cfg(test)]
use family_run::{
    estimate_total_matches, fsm_family_cache_path, FsmFamilyCacheEntry,
    DEFAULT_FAMILY_TM_MAX_STEPS, FSM_FAMILY_CACHE_SCHEMA_VERSION, MAX_FAMILY_RUN_MACHINES_CPU,
};

const DEFAULT_LOG_CAPACITY: usize = 512;

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

/// Sentinel value for `AgentsState::console_scroll` meaning "auto-scroll to bottom".
pub const CONSOLE_SCROLL_BOTTOM: usize = usize::MAX;

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

/// Queued claim-violation retry request. Populated by `agent_bus` when a
/// FileWrite auto-claim conflicts; drained by the TUI event loop which turns
/// each request into a corrective follow-up prompt for the violating agent.
#[derive(Clone, Debug)]
pub struct ClaimRetryRequest {
    pub agent_id: String,
    pub path: std::path::PathBuf,
    pub conflicting_holder: String,
    /// Rendered form of `ClaimKind` for the prompt (e.g. "ExclusiveWrite").
    pub conflicting_kind: String,
    pub conflicting_rationale: String,
}

/// Queued arbiter intervention. Produced by `arbiters::apply_interventions`
/// at tick boundaries (TurnCompleted, metabolism) and drained by the TUI
/// after `pending_claim_retries` — claims-first so an already-retrying
/// agent isn't doubly escalated. Shares `genome_retry_count` as its budget.
#[derive(Clone, Debug)]
pub struct Intervention {
    pub arbiter_name: &'static str,
    pub kind: crate::arbiters::InterventionKind,
    pub target: crate::arbiters::InterventionTarget,
    pub rationale: String,
    pub payload: serde_json::Value,
    pub decided_at_gen: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AppState {
    pub app_kind: AppKind,
    pub workspace_root: PathBuf,
    /// Directory names from `.gitignore` that should be excluded from file tracking and display.
    #[serde(skip)]
    pub gitignored_dirs: Vec<String>,
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
    /// Vim-style in-editor search (set by `*` / `#`, navigated by `n` / `N`).
    #[serde(skip)]
    pub editor_search: EditorSearch,
    /// Active `/` search prompt; `Some` while the user is typing a term.
    #[serde(skip)]
    pub search_prompt: Option<SearchPrompt>,
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
    /// In-memory hot tier of the genome report cache, hydrated at launch from
    /// the file-per-report disk tier under `.nit/genome/v1/<shard>/`. Read by
    /// the agent ops UI, dispatch landscape builder, and retry comparator.
    #[serde(skip)]
    pub genome_reports: crate::genome_report_cache::GenomeReportMap,
    /// Last genome diff text — surfaced to agents in retry prompts.
    #[serde(skip)]
    pub last_genome_diff: Option<String>,
    /// Direction of the last quality change: `+1` improved, `-1` degraded,
    /// `0` unchanged.
    #[serde(skip)]
    pub genome_quality_delta: i32,
    /// Baseline genome reports captured before the agent's first turn.
    /// Retries compare against THESE baselines, not the previous iteration —
    /// otherwise an agent that swings between equally bad shapes would
    /// never trip the regression check.
    #[serde(skip)]
    pub genome_baselines: crate::genome_report_cache::GenomeReportMap,
    /// Files modified during each agent's current turn.
    #[serde(skip)]
    pub genome_turn_modified: HashMap<String, HashSet<PathBuf>>,
    /// Per-mission accumulator (NOT cleared between turns) so that a
    /// swarm-end summary can see files modified across the whole mission,
    /// not just the final turn. Populated by the `FileWrite` handler when
    /// the writer has a `current_mission`; cleared on swarm start and
    /// follow-up reactivation.
    #[serde(skip)]
    pub genome_mission_modified: HashMap<String, HashSet<PathBuf>>,
    /// Agents currently in an active turn. File attribution rides on the
    /// runners' `FileWrite` events — not on filesystem polling.
    #[serde(skip)]
    pub genome_turn_active: HashSet<String>,
    /// `true` when a genome computation has been requested but not finished.
    #[serde(skip)]
    pub genome_computing: bool,
    /// Consecutive retry attempts for the current agent turn. Reset to 0
    /// when quality improves or stays the same. Kept as a scalar for
    /// display and non-agent-scoped paths (manual saves, shadow evals);
    /// parallel swarm code uses `genome_retry_counts` (per-agent) so a
    /// single noisy agent can't starve another's budget.
    #[serde(skip)]
    pub genome_retry_count: u8,
    /// Per-agent retry counters. Missing entry treated as 0.
    #[serde(skip)]
    pub genome_retry_counts: HashMap<String, u8>,
    /// Per-agent quality delta from the last authoritative evaluation batch.
    /// Read by `build_genome_retry_prompt` to decide whether to retry.
    /// Missing entry treated as 0 (no data → no retry).
    #[serde(skip)]
    pub genome_quality_deltas: HashMap<String, i32>,
    /// Claim-violation retry requests queued by `agent_bus` when a
    /// FileWrite auto-claim conflicts. Drained by the TUI event loop after
    /// each event-apply cycle; shares `genome_retry_count` as its budget.
    #[serde(skip)]
    pub pending_claim_retries: Vec<ClaimRetryRequest>,
    /// Arbiter interventions produced at tick boundaries. Drained after
    /// `pending_claim_retries`; shares `genome_retry_count` as budget.
    #[serde(skip)]
    pub pending_interventions: Vec<Intervention>,
    /// Consecutive turns where quality met or exceeded the agent's
    /// adaptive minimum tier. Drives the adaptive-threshold ladder up
    /// to Tier V (Replicator).
    #[serde(skip)]
    pub genome_agent_streak: HashMap<String, u8>,
    /// Effective minimum tier per agent, elevated by adaptive thresholds.
    #[serde(skip)]
    pub genome_agent_min_tier: HashMap<String, crate::genome_report::GenomeTier>,
    /// Real-time per-file shadow evaluation results during an active turn.
    /// Populated by the file watcher; cleared on `TurnStarted`.
    #[serde(skip)]
    pub genome_shadow_evals: HashMap<PathBuf, GenomeShadowEval>,
    /// Per-agent in-flight evaluation batches. Each completed turn opens
    /// one batch keyed by `agent_id`; results tagged with that id decrement
    /// `pending` until it reaches 0, then the retry decision fires for
    /// THAT agent. Keeping batches per-agent lets parallel swarm turns
    /// finalize independently — otherwise a later turn's
    /// `dispatch_turn_genome_evals` would clobber an earlier agent's slot.
    #[serde(skip)]
    pub genome_eval_batches: HashMap<String, GenomeEvalBatch>,
    /// Set by `Action::Save` to request a background genome evaluation.
    /// The TUI layer drains this and dispatches to `GenomeWorker`.
    #[serde(skip)]
    pub genome_save_eval_pending: Option<PathBuf>,
    /// Scroll offset for the gate monitor / structural quality pane.
    #[serde(skip)]
    pub gate_monitor_scroll: usize,
    /// Cached max_scroll for the gate monitor pane, updated per render by
    /// `gate_monitor_view::render`. Scroll handlers read this instead of
    /// rebuilding the full genome report on every wheel tick. `usize::MAX`
    /// is the "no render yet" sentinel.
    #[serde(skip, default = "gate_monitor_max_scroll_default")]
    pub gate_monitor_last_max_scroll: usize,
    /// Active sub-view for the structural quality pane: Stats or FileScores.
    #[serde(skip)]
    pub gate_monitor_sub_view: GateMonitorSubView,
    /// Whether the substrate inspector popup overlay is open.
    #[serde(skip)]
    pub show_substrate_overlay: bool,
    /// Active sub-tab inside the substrate overlay popup.
    #[serde(skip)]
    pub substrate_overlay_tab: SubstrateOverlayTab,
    /// Shared scroll offset for the substrate overlay body (all three sub-tabs).
    #[serde(skip)]
    pub substrate_overlay_scroll: usize,
    /// Cached max_scroll for the substrate overlay body, updated per render.
    /// Mirrors `gate_monitor_last_max_scroll`'s "no render yet" sentinel semantics.
    #[serde(skip, default = "gate_monitor_max_scroll_default")]
    pub substrate_overlay_last_max_scroll: usize,
    #[serde(default)]
    pub substrate: crate::substrate::SubstrateState,
    /// Multipane launch mode state. When `Some`, the standard single-pane
    /// run loop is never entered — the multipane event loop owns rendering
    /// and input. Per-launch only; not persisted.
    #[serde(skip)]
    pub multipane: Option<MultipaneState>,
}

/// Sub-view toggle for the CODE STRUCTURAL QUALITY pane.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GateMonitorSubView {
    #[default]
    Stats,
    /// Persisted reports from `.nit/genome/` — the whole workspace.
    FileScores,
    /// Files the workspace-wide scan is currently evaluating or has
    /// queued after a file-watcher invalidation. Session-local, shrinks
    /// to empty as the scan drains.
    Live,
}

/// Sub-tab inside the substrate inspector popup overlay.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SubstrateOverlayTab {
    #[default]
    Signals,
    Claims,
    Assumptions,
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

/// Construct the visualizer subtree of a fresh `AppState`. Conway-on-B3/S23
/// is the canonical default rule, and the seed-overlay defaults
/// (`halo` + `inset` ON, others OFF) match the first preset that
/// `cycle_seed_overlays` cycles through — so a freshly opened lab and a
/// post-cycle lab match.
fn default_visualizer_state(settings: &Settings) -> VisualizerState {
    VisualizerState {
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
    }
}

/// Construct the `GamesState` subtree of a fresh `AppState`. All popups
/// and `pending_*` flags start cleared — the petri dish starts idle and
/// only switches to `Running` once the operator dispatches `:games run`.
fn default_games_state() -> GamesState {
    GamesState {
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
    }
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
        let visualizer = default_visualizer_state(&settings);
        let games = default_games_state();
        Self {
            app_kind: AppKind::Gol,
            gitignored_dirs: Vec::new(),
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
            visualizer,
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
            games,
            file_tree,
            fuzzy_search,
            editor_search: EditorSearch::default(),
            search_prompt: None,
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
            genome_mission_modified: HashMap::new(),
            genome_turn_active: HashSet::new(),
            genome_retry_count: 0,
            genome_retry_counts: HashMap::new(),
            genome_quality_deltas: HashMap::new(),
            pending_claim_retries: Vec::new(),
            pending_interventions: Vec::new(),
            genome_agent_streak: HashMap::new(),
            genome_agent_min_tier: HashMap::new(),
            genome_shadow_evals: HashMap::new(),
            genome_eval_batches: HashMap::new(),
            genome_save_eval_pending: None,
            gate_monitor_scroll: 0,
            gate_monitor_last_max_scroll: usize::MAX,
            gate_monitor_sub_view: GateMonitorSubView::default(),
            show_substrate_overlay: false,
            substrate_overlay_tab: SubstrateOverlayTab::default(),
            substrate_overlay_scroll: 0,
            substrate_overlay_last_max_scroll: usize::MAX,
            substrate: crate::substrate::SubstrateState::default(),
            multipane: None,
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

#[cfg(test)]
#[path = "tests/state.rs"]
mod tests;
