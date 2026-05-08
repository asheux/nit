use super::*;

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

/// `last_max_scroll` is cached so wheel/keyboard scroll handlers can clamp
/// without calling `build_columns` (which re-runs the TM simulation and
/// rebuilds grid + rule tables) on every input event. `usize::MAX` is the
/// "no render yet" sentinel — see `last_max_scroll_sentinel` below.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    #[serde(skip, default = "scroll_cache_sentinel")]
    pub last_max_scroll: usize,
}

impl Default for GamesTmSimState {
    fn default() -> Self {
        Self {
            open: false,
            last_error: None,
            definition: None,
            input: None,
            steps_override: None,
            source_label: None,
            scroll_offset: 0,
            last_max_scroll: scroll_cache_sentinel(),
        }
    }
}

/// `last_max_scroll` mirrors `GamesTmSimState`'s render-cache sentinel —
/// `usize::MAX` means "no render yet, scroll position is unclamped".
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    #[serde(skip, default = "scroll_cache_sentinel")]
    pub last_max_scroll: usize,
}

impl Default for GamesCaSimState {
    fn default() -> Self {
        Self {
            open: false,
            last_error: None,
            definition: None,
            input: None,
            steps_override: None,
            source_label: None,
            scroll_offset: 0,
            last_max_scroll: scroll_cache_sentinel(),
        }
    }
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

pub(super) fn open_games_history_popup(state: &mut AppState) {
    let games = &mut state.games;
    let history = &mut games.match_history;

    // Lazy-populate totals when entries arrived without going through the
    // capture pipeline (e.g. opened from a previously-loaded run).
    if history.total_entries == 0 && !history.entries.is_empty() {
        history.total_entries = history.entries.len();
        history.loaded_start = 0;
        history.max_rounds_seen = history
            .entries
            .iter()
            .map(|entry| entry.rounds_total as usize)
            .max()
            .unwrap_or(0);
    }

    history.last_error = if history.total_entries == 0 && !history.capture_disabled_for_run {
        Some("No completed matches available yet. Start a tournament first.".into())
    } else {
        None
    };

    games.run_browser.open = false;
    games.replay.open = false;
    games.strategy_inspect.open = false;
    games.analysis.open = false;
    games.tm_sim.open = false;
    games.ca_sim.open = false;
    games.match_history.open = true;
    games.match_history.column_offset = 0;
    games.match_history.round_limit = None;
    state.status = Some("Games match history plot opened".into());
}
