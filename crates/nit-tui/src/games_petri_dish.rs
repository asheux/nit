use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
#[cfg(target_os = "macos")]
use std::sync::Once;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{
    apply_family_run_runtime_overrides, build_family_run_override_for_request_with_timings,
    build_family_run_override_from_base_config_with_timings, AppState, FamilyRunBuildTimings,
    GamesAnalysisRequest, GamesFamilyRunRequest, GamesRunOverride, GamesStatus, UiSelectionPane,
};
use nit_games::output::StrategyDefinition;
use nit_games::{
    events::EventWriter,
    output::{RunLayout, TournamentResults},
    run_id_from_seed_config, FastStrategyModel, MatchSnapshot, TournamentProgress,
};
use nit_metal::BatchPolicyCacheSnapshot;
use nit_utils::hashing::stable_hash_bytes;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use tracing::info;

use crate::games_analysis::{AnalysisCommand, AnalysisEvent, AnalysisRequest, GamesAnalysisRunner};
use crate::games_runner::{
    uses_match_step_units, GamesRunner, RunRequest, RunnerCommand, RunnerEvent,
};
use crate::games_runs::{GamesRunsRunner, RunsCommand, RunsEvent};
use crate::theme::Theme;
use crate::widgets::games_visualizer_view::strategy_display_name_from_def;
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 120;
const MIN_HEIGHT: u16 = 40;
const HISTORY_LOAD_CHUNK: usize = 256;
const HISTORY_LOAD_PREFETCH: usize = 64;
const ROUND_STEP_UI_CAP: u32 = 200;
const MATCH_STEP_UI_CAP: u32 = 250_000;
#[cfg(target_os = "macos")]
static METAL_PREWARM_ONCE: Once = Once::new();

pub fn petri_rect(screen: Rect) -> Rect {
    let width = screen.width.clamp(60, MIN_WIDTH);
    let height = screen.height.clamp(16, MIN_HEIGHT);
    Rect {
        x: screen.x + (screen.width.saturating_sub(width)) / 2,
        y: screen.y + (screen.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PetriView {
    Tournament,
    Inspector,
    Cache,
}

pub struct GamesPetriDishRuntime {
    runner: GamesRunner,
    analysis_runner: GamesAnalysisRunner,
    runs_runner: GamesRunsRunner,
    session: Option<GameSession>,
    history_store: Option<HistoryPreviewStore>,
    hidden: bool,
    warning: Option<String>,
    confirm_clear_all_cache: bool,
    last_tick: Instant,
    view: PetriView,
    inspector_window: usize,
    cache_snapshot: BatchPolicyCacheSnapshot,
    cache_selected: usize,
    family_run_rx: Option<Receiver<FamilyBuildOutcome>>,
    loading_open: bool,
    loading_message: Option<String>,
    activity_epoch: u64,
}

struct GameSession {
    config: nit_games::NormalizedConfig,
    progress: Option<TournamentProgress>,
    snapshot: Option<MatchSnapshot>,
    results: TournamentResults,
    definitions: Vec<StrategyDefinition>,
    started_at: Instant,
    finished_elapsed: Option<Duration>,
}

impl GameSession {
    fn elapsed(&self) -> Duration {
        self.elapsed_at(Instant::now())
    }

    fn elapsed_at(&self, now: Instant) -> Duration {
        self.finished_elapsed
            .unwrap_or_else(|| now.saturating_duration_since(self.started_at))
    }

    fn freeze_elapsed(&mut self) -> Duration {
        let elapsed = self.started_at.elapsed();
        *self.finished_elapsed.get_or_insert(elapsed)
    }
}

struct FamilyBuildOutcome {
    force: bool,
    result: Result<GamesRunOverride, String>,
    timings: Option<FamilyRunBuildTimings>,
}

fn family_build_detail(family: &str, raw_input: &str) -> String {
    let trimmed = raw_input.trim();
    let detail = trimmed
        .find('{')
        .map(|start| trimmed[start..].trim())
        .filter(|detail| !detail.is_empty())
        .unwrap_or(trimmed);
    if detail.is_empty() {
        family.to_string()
    } else {
        format!("{family} {detail}")
    }
}

fn tm_family_prep_summary(timings: &FamilyRunBuildTimings) -> Option<String> {
    let diagnostics = timings.tm_filter.as_ref()?;
    let summary = match diagnostics.backend {
        nit_games::TmHaltingFilterBackend::Metal => "prep used Metal".to_string(),
        nit_games::TmHaltingFilterBackend::NotebookCpuFallback => {
            if let Some(reason) = diagnostics.metal_decline_reason.as_ref() {
                format!("prep fell back to CPU: {reason}")
            } else if let Some(err) = diagnostics.metal_error.as_ref() {
                format!("prep fell back to CPU after Metal error: {err}")
            } else {
                "prep fell back to CPU".to_string()
            }
        }
        nit_games::TmHaltingFilterBackend::NotebookCpu => "prep used CPU".to_string(),
        other => format!("prep backend {}", other.label()),
    };
    Some(summary)
}

struct HistoryPreviewStore {
    path: PathBuf,
    writer: BufWriter<File>,
    wolfram_writer: BufWriter<File>,
    offsets: Vec<u64>,
    next_offset: u64,
}

impl HistoryPreviewStore {
    fn create(path: PathBuf) -> std::io::Result<Self> {
        let file = File::create(&path)?;
        let wolfram_file = File::create(path.with_extension("wl"))?;
        Ok(Self {
            path,
            writer: BufWriter::new(file),
            wolfram_writer: BufWriter::new(wolfram_file),
            offsets: Vec::new(),
            next_offset: 0,
        })
    }

    fn push(&mut self, preview: &nit_games::MatchHistoryPreview) -> std::io::Result<()> {
        let encoded = serde_json::to_vec(preview).map_err(std::io::Error::other)?;
        let wolfram = wolfram_preview_line(preview);
        self.offsets.push(self.next_offset);
        self.writer.write_all(&encoded)?;
        self.writer.write_all(b"\n")?;
        self.wolfram_writer.write_all(wolfram.as_bytes())?;
        self.wolfram_writer.write_all(b"\n")?;
        self.next_offset = self
            .next_offset
            .saturating_add(encoded.len() as u64)
            .saturating_add(1);
        Ok(())
    }

    fn len(&self) -> usize {
        self.offsets.len()
    }

    fn load_range(
        &mut self,
        start: usize,
        count: usize,
    ) -> std::io::Result<Vec<nit_games::MatchHistoryPreview>> {
        if count == 0 || start >= self.offsets.len() {
            return Ok(Vec::new());
        }
        self.writer.flush()?;
        self.wolfram_writer.flush()?;
        let end = start.saturating_add(count).min(self.offsets.len());
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offsets[start]))?;
        let mut reader = BufReader::new(file);
        let mut entries = Vec::with_capacity(end.saturating_sub(start));
        let mut line = String::new();
        for _ in start..end {
            line.clear();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                break;
            }
            let trimmed = line.trim_end_matches(&['\n', '\r'][..]);
            let preview = serde_json::from_str::<nit_games::MatchHistoryPreview>(trimmed)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
            entries.push(preview);
        }
        Ok(entries)
    }
}

fn wolfram_preview_line(preview: &nit_games::MatchHistoryPreview) -> String {
    format!(
        "<|\"match_index\" -> {}, \"total_matches\" -> {}, \"a\" -> \"{}\", \"b\" -> \"{}\", \"rounds_total\" -> {}, \"outcomes\" -> \"{}\"|>",
        preview.match_index,
        preview.total_matches,
        escape_wolfram_string(&preview.a),
        escape_wolfram_string(&preview.b),
        preview.rounds_total,
        escape_wolfram_string(&preview.outcomes),
    )
}

fn escape_wolfram_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

fn increase_steps_per_tick(current: u32, use_match_units: bool) -> u32 {
    let base_cap = if use_match_units {
        MATCH_STEP_UI_CAP
    } else {
        ROUND_STEP_UI_CAP
    };
    let next = current.saturating_add(1);
    if current > base_cap {
        next
    } else {
        next.min(base_cap).max(1)
    }
}

fn decrease_steps_per_tick(current: u32) -> u32 {
    current.saturating_sub(1).max(1)
}

impl GamesPetriDishRuntime {
    pub fn new(_state: &AppState) -> Self {
        #[cfg(target_os = "macos")]
        METAL_PREWARM_ONCE.call_once(|| {
            let _ = thread::Builder::new()
                .name("nit-metal-prewarm".into())
                .spawn(|| {
                    let _ = nit_metal::prewarm_default_batch_shaders();
                });
        });
        Self {
            runner: GamesRunner::spawn(),
            analysis_runner: GamesAnalysisRunner::spawn(),
            runs_runner: GamesRunsRunner::spawn(),
            session: None,
            history_store: None,
            hidden: false,
            warning: None,
            confirm_clear_all_cache: false,
            last_tick: Instant::now(),
            view: PetriView::Tournament,
            inspector_window: 50,
            cache_snapshot: BatchPolicyCacheSnapshot::default(),
            cache_selected: 0,
            family_run_rx: None,
            loading_open: false,
            loading_message: None,
            activity_epoch: 0,
        }
    }

    pub fn activity_epoch(&self) -> u64 {
        self.activity_epoch
    }

    fn mark_activity(&mut self) {
        self.activity_epoch = self.activity_epoch.wrapping_add(1);
    }

    fn refresh_cache_snapshot(&mut self, state: &mut AppState) {
        match nit_metal::batch_policy_cache_snapshot() {
            Ok(snapshot) => {
                if let Some(active_path) = state.games.runtime.metal_policy_cache_path.as_ref() {
                    if let Some(idx) = snapshot
                        .entries
                        .iter()
                        .position(|entry| &entry.path == active_path)
                    {
                        self.cache_selected = idx;
                    } else {
                        self.cache_selected = self
                            .cache_selected
                            .min(snapshot.entries.len().saturating_sub(1));
                    }
                } else {
                    self.cache_selected = self
                        .cache_selected
                        .min(snapshot.entries.len().saturating_sub(1));
                }
                self.cache_snapshot = snapshot;
            }
            Err(err) => {
                self.warning = Some(format!("Metal cache refresh failed: {err}"));
            }
        }
    }

    fn open_cache_view(&mut self, state: &mut AppState) {
        self.refresh_cache_snapshot(state);
        self.view = PetriView::Cache;
    }

    fn adjust_cache_selection(&mut self, delta: isize) {
        let len = self.cache_snapshot.entries.len();
        if len == 0 {
            self.cache_selected = 0;
            return;
        }
        let next = (self.cache_selected as isize + delta).clamp(0, len.saturating_sub(1) as isize);
        self.cache_selected = next as usize;
    }

    fn remove_selected_cache_entry(&mut self, state: &mut AppState) {
        let Some(entry) = self.cache_snapshot.entries.get(self.cache_selected) else {
            state.status = Some("No Metal cache entry selected".into());
            return;
        };
        match nit_metal::clear_batch_policy_cache_entry(&entry.path) {
            Ok(true) => {
                state.status = Some(format!("Removed Metal cache entry {}", entry.key));
                self.refresh_cache_snapshot(state);
            }
            Ok(false) => {
                state.status = Some("Metal cache entry was already gone".into());
                self.refresh_cache_snapshot(state);
            }
            Err(err) => {
                self.warning = Some(format!("Metal cache delete failed: {err}"));
            }
        }
    }

    fn clear_all_cache_entries(&mut self, state: &mut AppState) {
        match nit_metal::clear_batch_policy_cache() {
            Ok(0) => {
                state.status = Some("Metal cache was already empty".into());
                self.refresh_cache_snapshot(state);
            }
            Ok(removed) => {
                state.status = Some(format!(
                    "Removed {removed} Metal cache entr{}",
                    if removed == 1 { "y" } else { "ies" }
                ));
                self.refresh_cache_snapshot(state);
            }
            Err(err) => {
                self.warning = Some(format!("Metal cache clear failed: {err}"));
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some() || self.loading_open
    }

    pub fn is_visible(&self) -> bool {
        (self.session.is_some() || self.loading_open) && !self.hidden
    }

    pub fn handle_pending_requests(&mut self, state: &mut AppState) {
        self.handle_family_build_result(state);
        if let Some(request) = state.games.pending_family_run.take() {
            self.start_family_build(state, request);
        }
        if state.games.pending_close {
            state.games.pending_close = false;
            self.close(state);
        }
        if state.games.pending_hide {
            state.games.pending_hide = false;
            self.hide(state);
        }
        if state.games.pending_show {
            state.games.pending_show = false;
            self.show(state);
        }
        if state.games.pending_run {
            state.games.pending_run = false;
            self.start_session(state);
        }
        if state.games.pending_export {
            state.games.pending_export = false;
            self.export_last_run(state);
        }
        if let Some(request) = state.games.pending_analyze.take() {
            self.start_analysis(state, request);
        }
        if state.games.pending_run_browser {
            state.games.pending_run_browser = false;
            state.games.run_browser.loading = true;
            state.games.run_browser.last_error = None;
            self.runs_runner.send(RunsCommand::Refresh {
                base_dir: state.workspace_root.clone(),
            });
        }
        if let Some(path) = state.games.pending_run_load.take() {
            let mut summary_path = PathBuf::from(path);
            if summary_path.is_relative() {
                summary_path = state.workspace_root.join(summary_path);
            }
            state.games.run_browser.loading = true;
            state.games.run_browser.last_error = None;
            self.runs_runner
                .send(RunsCommand::LoadSummary { summary_path });
        }
        if let Some(request) = state.games.pending_replay.take() {
            let Some(run) = state.games.last_run.as_ref() else {
                state.games.replay.loading = false;
                state.games.replay.last_error = Some("No run loaded for replay".into());
                return;
            };
            let history_path = run.history_log.clone().or(run.paths.history.clone());
            let Some(path) = history_path else {
                state.games.replay.loading = false;
                state.games.replay.last_error = Some("Run has no history log".into());
                return;
            };
            let mut history_path = PathBuf::from(path);
            if history_path.is_relative() {
                history_path = state.workspace_root.join(history_path);
            }
            state.games.replay.loading = true;
            state.games.replay.last_error = None;
            self.runs_runner.send(RunsCommand::LoadReplay {
                history_path,
                a_id: request.a_id,
                b_id: request.b_id,
                payoff: run.config.payoff,
            });
        }
    }

    fn start_family_build(&mut self, state: &mut AppState, request: GamesFamilyRunRequest) {
        if self.family_run_rx.is_some() {
            state.games.family_building = true;
            state.status = Some("Family run preparation already in progress".into());
            return;
        }
        let workspace_root = state.workspace_root.clone();
        let config_text = state.editor_buffer().content_as_string();
        let config_version = state.editor_buffer().version();
        let base_config = state
            .games
            .config_preview
            .as_ref()
            .filter(|preview| preview.version == config_version)
            .and_then(|preview| preview.result.as_ref().ok())
            .map(nit_games::config::FamilyRunBaseConfig::from_normalized);
        let used_preview_base = base_config.is_some();
        let family_label = request.family.clone();
        let mode = if request.force { "forced, " } else { "" };
        let force = request.force;
        state.games.family_building = true;
        let detail = family_build_detail(&family_label, &request.input);
        self.open_loading_popup(state, format!("Preparing family run ({mode}{detail})..."));
        state.status = None;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let build_started = Instant::now();
            let built = match base_config {
                Some(base_config) => build_family_run_override_from_base_config_with_timings(
                    &workspace_root,
                    &config_text,
                    &request,
                    base_config,
                ),
                None => build_family_run_override_for_request_with_timings(
                    &workspace_root,
                    &config_text,
                    &request,
                ),
            };
            let (result, timings) = match built {
                Ok((override_run, timings)) => {
                    let tm_prep = tm_family_prep_summary(&timings)
                        .unwrap_or_else(|| "no TM prep diagnostics".to_string());
                    info!(
                        "Games family build ({family_label}) generated {} strategies in {:?} (generation {:?}, estimate {:?}, normalize {:?}, tm-filter {:?}) using {} base config; {}",
                        timings.generated_strategies,
                        timings.total_elapsed,
                        timings.generation_elapsed,
                        timings.estimate_elapsed,
                        timings.normalize_elapsed,
                        timings.tm_filter_elapsed,
                        if used_preview_base {
                            "preview"
                        } else {
                            "parsed"
                        },
                        tm_prep
                    );
                    (Ok(override_run), Some(timings))
                }
                Err(err) => {
                    info!(
                        "Games family build ({family_label}) failed after {:?} using {} base config: {}",
                        build_started.elapsed(),
                        if used_preview_base {
                            "preview"
                        } else {
                            "parsed"
                        },
                        err
                    );
                    (Err(err), None)
                }
            };
            let _ = tx.send(FamilyBuildOutcome {
                force,
                result,
                timings,
            });
        });
        self.family_run_rx = Some(rx);
    }

    fn handle_family_build_result(&mut self, state: &mut AppState) {
        let Some(rx) = self.family_run_rx.as_ref() else {
            return;
        };
        if !state.games.family_building {
            self.family_run_rx = None;
            return;
        }
        let outcome = match rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(FamilyBuildOutcome {
                force: false,
                result: Err("Family run preparation failed".into()),
                timings: None,
            }),
        };
        let Some(outcome) = outcome else {
            return;
        };
        self.family_run_rx = None;
        state.games.family_building = false;
        match outcome.result {
            Ok(override_run) => {
                let machine_count = override_run.config.strategies.len();
                let label = override_run.label.clone();
                let mode = if outcome.force { "forced, " } else { "" };
                state.games.pending_run_override = Some(override_run);
                state.games.pending_run = true;
                self.loading_message = Some(match outcome.timings {
                    Some(timings) => {
                        let mut message = format!(
                            "Queued tournament ({mode}{label}, {machine_count} machines; generation {:?}, estimate {:?}, normalize {:?}",
                            timings.generation_elapsed,
                            timings.estimate_elapsed,
                            timings.normalize_elapsed
                        );
                        if let Some(tm_filter_elapsed) = timings.tm_filter_elapsed {
                            message.push_str(&format!(", tm-filter {:?}", tm_filter_elapsed));
                        }
                        message.push(')');
                        if let Some(tm_summary) = tm_family_prep_summary(&timings) {
                            message.push_str(&format!("; {tm_summary}"));
                        }
                        message
                    }
                    None => format!("Queued tournament ({mode}{label}, {machine_count} machines)"),
                });
                state.status = None;
            }
            Err(err) => {
                state.games.pending_run_override = None;
                state.games.pending_run = false;
                self.loading_open = false;
                self.loading_message = None;
                state.games.running = false;
                state.status = Some(err);
            }
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent, state: &mut AppState) -> bool {
        if self.confirm_clear_all_cache {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.confirm_clear_all_cache = false;
                    self.clear_all_cache_entries(state);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_clear_all_cache = false;
                    state.status = Some("Metal cache clear cancelled".into());
                }
                _ => {}
            }
            return true;
        }
        if self.warning.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ')) {
                self.warning = None;
                return true;
            }
            return true;
        }
        if self.session.is_none() {
            if !self.loading_open {
                return false;
            }
            return match key.code {
                KeyCode::Esc => {
                    self.close(state);
                    true
                }
                KeyCode::Char('h') | KeyCode::Char('H') => {
                    self.hide(state);
                    true
                }
                _ => false,
            };
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl
            && matches!(
                key.code,
                KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r')
            )
        {
            if state.games.paused {
                self.runner.send(RunnerCommand::StepOnce);
            }
            return true;
        }

        match key.code {
            KeyCode::Tab => {
                self.view = match self.view {
                    PetriView::Tournament => PetriView::Inspector,
                    PetriView::Inspector | PetriView::Cache => PetriView::Tournament,
                };
                true
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.open_cache_view(state);
                true
            }
            KeyCode::Left => {
                if self.view == PetriView::Inspector {
                    self.adjust_inspector_window(-1);
                    true
                } else {
                    false
                }
            }
            KeyCode::Right => {
                if self.view == PetriView::Inspector {
                    self.adjust_inspector_window(1);
                    true
                } else {
                    false
                }
            }
            KeyCode::Up => {
                if self.view == PetriView::Cache {
                    self.adjust_cache_selection(-1);
                    true
                } else {
                    false
                }
            }
            KeyCode::Down => {
                if self.view == PetriView::Cache {
                    self.adjust_cache_selection(1);
                    true
                } else {
                    false
                }
            }
            KeyCode::Esc => {
                self.close(state);
                true
            }
            KeyCode::Char(' ') | KeyCode::Null | KeyCode::Char('\u{0}') => {
                state.games.paused = !state.games.paused;
                state.games.status = if state.games.paused {
                    GamesStatus::Paused
                } else {
                    GamesStatus::Running
                };
                if state.games.paused {
                    self.runner.send(RunnerCommand::Pause);
                } else {
                    self.runner.send(RunnerCommand::Resume);
                }
                true
            }
            KeyCode::Enter => {
                if state.games.paused {
                    self.runner.send(RunnerCommand::StepOnce);
                }
                true
            }
            KeyCode::Char('h') | KeyCode::Char('H') => {
                self.hide(state);
                true
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                state.games.steps_per_tick = increase_steps_per_tick(
                    state.games.steps_per_tick,
                    state.games.steps_use_match_units,
                );
                self.runner
                    .send(RunnerCommand::UpdateSpeed(state.games.steps_per_tick));
                true
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                state.games.steps_per_tick = decrease_steps_per_tick(state.games.steps_per_tick);
                self.runner
                    .send(RunnerCommand::UpdateSpeed(state.games.steps_per_tick));
                true
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if self.view == PetriView::Cache {
                    self.refresh_cache_snapshot(state);
                    state.status = Some("Metal cache refreshed".into());
                    true
                } else {
                    false
                }
            }
            KeyCode::Char('x') => {
                if self.view == PetriView::Cache {
                    self.remove_selected_cache_entry(state);
                    true
                } else {
                    false
                }
            }
            KeyCode::Char('X') => {
                if self.view == PetriView::Cache {
                    self.confirm_clear_all_cache = true;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub fn tick(&mut self, state: &mut AppState) {
        self.handle_analysis_events(state);
        self.handle_runs_events(state);
        self.handle_runner_events(state);
        self.refresh_history_window(state);
        self.last_tick = Instant::now();
    }

    fn handle_analysis_events(&mut self, state: &mut AppState) {
        while let Ok(event) = self.analysis_runner.events.try_recv() {
            self.mark_activity();
            match event {
                AnalysisEvent::Started(request) => {
                    let request = *request;
                    state.games.analysis.running = true;
                    state.games.analysis.last_error = None;
                    state.games.analysis.source_path =
                        Some(request.history_path.display().to_string());
                    state.status = Some("Games analysis started".into());
                }
                AnalysisEvent::Finished(result) => {
                    let result = *result;
                    state.games.analysis.running = false;
                    state.games.analysis.last_error = None;
                    state.games.analysis.summary = Some(result.summary);
                    state.games.analysis.preview = Some(result.preview);
                    state.status = Some("Games analysis complete".into());
                }
                AnalysisEvent::Error(err) => {
                    state.games.analysis.running = false;
                    state.games.analysis.last_error = Some(err.clone());
                    state.status = Some(err);
                }
            }
        }
    }

    fn handle_runs_events(&mut self, state: &mut AppState) {
        while let Ok(event) = self.runs_runner.events.try_recv() {
            self.mark_activity();
            match event {
                RunsEvent::RunsLoaded(entries) => {
                    state.games.run_browser.loading = false;
                    state.games.run_browser.last_error = None;
                    state.games.run_browser.entries = entries;
                    state.games.run_browser.selected = 0;
                    state.games.run_browser.scroll_offset = 0;
                    if state.games.run_browser.entries.is_empty() {
                        state.games.run_browser.last_error =
                            Some("No runs found in runs/games".into());
                    }
                }
                RunsEvent::SummaryLoaded(summary) => {
                    let summary = *summary;
                    let pairs = summary
                        .results
                        .pairwise
                        .iter()
                        .map(|p| (p.a.clone(), p.b.clone()))
                        .collect::<Vec<_>>();
                    state.games.last_run_path = summary.paths.summary.clone();
                    state.games.last_event_path = summary.event_log.clone();
                    state.games.last_history_path = summary.history_log.clone();
                    state.games.runtime = summary.runtime.clone();
                    state.games.last_run = Some(summary);
                    state.games.replay.pairs = pairs;
                    state.games.replay.title = None;
                    state.games.replay.lines.clear();
                    state.games.replay.cycle = None;
                    state.games.replay.selected_pair = None;
                    state.games.replay.selected_index = 0;
                    state.games.replay.scroll_offset = 0;
                    reset_strategy_inspect(state);
                    state.games.run_browser.loading = false;
                    state.games.run_browser.open = false;
                    if let Some(selection) = state.ui_selection {
                        if matches!(selection.pane, UiSelectionPane::GamesRunBrowserPopup) {
                            state.ui_selection = None;
                        }
                    }
                    state.status = Some("Run summary loaded".into());
                }
                RunsEvent::ReplayLoaded(replay) => {
                    state.games.replay.loading = false;
                    state.games.replay.last_error = None;
                    state.games.replay.title = Some(replay.title);
                    state.games.replay.lines = replay.lines;
                    state.games.replay.cycle = replay.cycle;
                    state.games.replay.scroll_offset = 0;
                }
                RunsEvent::Error(err) => {
                    if state.games.run_browser.open && state.games.run_browser.loading {
                        state.games.run_browser.loading = false;
                        state.games.run_browser.last_error = Some(err.clone());
                    } else if state.games.replay.open && state.games.replay.loading {
                        state.games.replay.loading = false;
                        state.games.replay.last_error = Some(err.clone());
                    } else {
                        state.status = Some(err);
                    }
                }
            }
        }
    }

    fn handle_runner_events(&mut self, state: &mut AppState) {
        while let Ok(event) = self.runner.events.try_recv() {
            self.mark_activity();
            match event {
                RunnerEvent::StartupStage(stage) => {
                    self.loading_open = true;
                    self.loading_message = Some(stage);
                }
                RunnerEvent::Definitions(defs) => {
                    if let Some(session) = self.session.as_mut() {
                        session.definitions = defs;
                    }
                    self.loading_open = false;
                    self.loading_message = None;
                }
                RunnerEvent::Progress(progress) => {
                    state.games.match_history.total_entries = state
                        .games
                        .match_history
                        .total_entries
                        .max(progress.total_matches);
                    state.games.runtime = progress.runtime.clone();
                    if let Some(session) = self.session.as_mut() {
                        session.progress = Some(progress);
                    }
                }
                RunnerEvent::MatchPreview(snapshot) => {
                    if let Some(session) = self.session.as_mut() {
                        session.snapshot = Some(snapshot);
                    }
                }
                RunnerEvent::MatchHistoryPreview(preview) => {
                    state.games.match_history.max_rounds_seen = state
                        .games
                        .match_history
                        .max_rounds_seen
                        .max(preview.rounds_total as usize)
                        .max(preview.outcomes.len());
                    if let Some(store) = self.history_store.as_mut() {
                        if let Err(err) = store.push(&preview) {
                            state.games.match_history.last_error =
                                Some(format!("Failed to cache match history preview: {err}"));
                            state.games.match_history.entries.push(preview);
                            state.games.match_history.loaded_start = 0;
                            state.games.match_history.total_entries = state
                                .games
                                .match_history
                                .total_entries
                                .max(state.games.match_history.entries.len());
                        } else {
                            state.games.match_history.total_entries =
                                state.games.match_history.total_entries.max(store.len());
                        }
                    } else {
                        state.games.match_history.entries.push(preview);
                        state.games.match_history.loaded_start = 0;
                        state.games.match_history.total_entries = state
                            .games
                            .match_history
                            .total_entries
                            .max(state.games.match_history.entries.len());
                    }
                }
                RunnerEvent::PartialLeaderboard(results) => {
                    if let Some(session) = self.session.as_mut() {
                        session.results = results;
                    }
                }
                RunnerEvent::Finished(summary) => {
                    self.finish_session(state, *summary);
                }
                RunnerEvent::Cancelled => {
                    let elapsed = self.session.as_mut().map(GameSession::freeze_elapsed);
                    self.session = None;
                    self.loading_open = false;
                    self.loading_message = None;
                    state.games.last_error = None;
                    state.games.running = false;
                    state.games.paused = false;
                    state.games.steps_use_match_units = false;
                    state.games.status = GamesStatus::Idle;
                    state.games.petri_hidden = false;
                    state.games.petri_lines.clear();
                    state.status = Some(match elapsed {
                        Some(elapsed) => format!(
                            "Games tournament cancelled after {}",
                            format_tournament_elapsed(elapsed)
                        ),
                        None => "Games tournament cancelled".into(),
                    });
                }
                RunnerEvent::Error(err) => {
                    let elapsed = self.session.as_mut().map(GameSession::freeze_elapsed);
                    self.session = None;
                    self.loading_open = false;
                    self.loading_message = None;
                    state.games.running = false;
                    state.games.paused = false;
                    state.games.steps_use_match_units = false;
                    state.games.status = GamesStatus::Error;
                    state.games.last_error = Some(err.clone());
                    state.games.petri_lines.clear();
                    state.status = Some(match elapsed {
                        Some(elapsed) => {
                            format!("{err} (after {})", format_tournament_elapsed(elapsed))
                        }
                        None => err,
                    });
                }
            }
        }
    }

    fn finish_session(&mut self, state: &mut AppState, summary: nit_games::output::RunSummary) {
        let pairs = summary
            .results
            .pairwise
            .iter()
            .map(|p| (p.a.clone(), p.b.clone()))
            .collect::<Vec<_>>();
        let elapsed = if let Some(session) = self.session.as_mut() {
            let elapsed = session.freeze_elapsed();
            session.results = summary.results.clone();
            session.definitions = summary.strategies.clone();
            Some(elapsed)
        } else {
            None
        };
        state.games.last_error = None;
        state.games.last_run_path = summary.paths.summary.clone();
        state.games.last_event_path = summary.event_log.clone();
        state.games.last_history_path = summary.history_log.clone();
        state.games.runtime = summary.runtime.clone();
        state.games.last_run = Some(summary);
        state.games.replay.pairs = pairs;
        reset_strategy_inspect(state);
        state.games.status = GamesStatus::Done;
        state.games.running = false;
        state.games.paused = false;
        state.games.steps_use_match_units = false;
        self.hidden = false;
        state.games.petri_hidden = false;
        state.games.petri_lines.clear();
        state.status = Some(match elapsed {
            Some(elapsed) => format!(
                "Games tournament completed in {}",
                format_tournament_elapsed(elapsed)
            ),
            None => "Games tournament completed".into(),
        });
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect, state: &mut AppState, theme: &Theme) {
        if !self.is_visible() {
            return;
        }
        let area = petri_rect(screen);
        frame.render_widget(Clear, area);

        let border_style = Style::default().fg(theme.border);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(theme.background))
            .title(Span::styled(
                " GAMES PETRI DISH ",
                Style::default()
                    .fg(theme.title_focused)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let value_style = Style::default().fg(theme.foreground);
        let header_style = Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD);
        let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
        let dim_style = Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);
        let number_style = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD);
        let win_style = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD);
        let loss_style = Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD);
        let draw_style = Style::default().fg(theme.title);
        let key_style = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD);
        let help_style = Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);

        let mut lines = Vec::new();
        if self.confirm_clear_all_cache {
            lines.push(Line::from(Span::styled(
                "Confirm clearing all Metal cache entries?",
                Style::default().fg(theme.warning),
            )));
            lines.push(Line::from(vec![
                Span::styled("Y", key_style),
                Span::styled(" / ", help_style),
                Span::styled("Enter", key_style),
                Span::styled(" confirm | ", help_style),
                Span::styled("N", key_style),
                Span::styled(" / ", help_style),
                Span::styled("Esc", key_style),
                Span::styled(" cancel", help_style),
            ]));
        } else if let Some(warning) = self.warning.as_ref() {
            lines.push(Line::from(Span::styled(
                warning.clone(),
                Style::default().fg(theme.warning),
            )));
        } else if self.loading_open {
            let fallback = if state.games.family_building {
                Some("Preparing family run...")
            } else if state.games.pending_run {
                Some("Preparing run config...")
            } else {
                None
            };
            let loading_message = self
                .loading_message
                .as_deref()
                .or(fallback)
                .unwrap_or("Starting tournament...");
            lines.push(Line::from(vec![
                Span::styled("Status: ", label_style),
                Span::styled("Loading", status_style(state, theme)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Stage: ", label_style),
                Span::styled(loading_message.to_string(), value_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Match: ", label_style),
                Span::styled("Waiting for runner...", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Pair: ", label_style),
                Span::styled("Waiting for runner...", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Last: ", label_style),
                Span::styled("Waiting for runner...", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Halt: ", label_style),
                Span::styled("Waiting for runner...", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Payoff: ", label_style),
                Span::styled("Waiting for runner...", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Total: ", label_style),
                Span::styled("Waiting for runner...", dim_style),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Esc", key_style),
                Span::styled(" close | ", help_style),
                Span::styled("H", key_style),
                Span::styled(" hide", help_style),
            ]));
        } else if let Some(session) = self.session.as_ref() {
            let elapsed = session.elapsed();
            match self.view {
                PetriView::Tournament => {
                    let progress = session.progress.clone();
                    lines.extend(render_progress(
                        progress,
                        &session.definitions,
                        state,
                        label_style,
                        value_style,
                        number_style,
                        dim_style,
                        status_style(state, theme),
                    ));
                    lines.push(Line::from(""));
                    let results = &session.results;
                    let definitions = &session.definitions;
                    let (rank_w, score_w, total_w, wld_w) = top_table_widths(&session.config);
                    lines.extend(render_top_table(
                        results,
                        &session.config,
                        definitions,
                        inner.width as usize,
                        header_style,
                        label_style,
                        value_style,
                        number_style,
                        win_style,
                        loss_style,
                        draw_style,
                        dim_style,
                        rank_w,
                        score_w,
                        total_w,
                        wld_w,
                    ));
                }
                PetriView::Inspector => {
                    let snapshot = session.snapshot.clone();
                    let progress = session.progress.clone();
                    lines.extend(render_match_inspector(
                        snapshot,
                        progress,
                        &session.definitions,
                        state.games.status,
                        self.inspector_window,
                        inner.width as usize,
                        header_style,
                        label_style,
                        value_style,
                        number_style,
                        dim_style,
                        loss_style,
                    ));
                }
                PetriView::Cache => {
                    lines.extend(render_cache_browser(
                        &self.cache_snapshot,
                        self.cache_selected,
                        inner.width as usize,
                        header_style,
                        label_style,
                        value_style,
                        dim_style,
                        key_style,
                    ));
                }
            }
            lines.push(Line::from(""));
            let paused_style = if state.games.paused {
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.accent)
            };
            lines.push(session_footer_line(
                state.games.steps_per_tick,
                state.games.steps_use_match_units,
                state.games.paused,
                elapsed,
                label_style,
                number_style,
                paused_style,
                dim_style,
            ));
            lines.push(Line::from(match self.view {
                PetriView::Tournament => vec![
                    Span::styled("Esc", key_style),
                    Span::styled(" close | ", help_style),
                    Span::styled("Space", key_style),
                    Span::styled(" pause | ", help_style),
                    Span::styled("Enter", key_style),
                    Span::styled(" step | ", help_style),
                    Span::styled("+/-", key_style),
                    Span::styled(" speed | ", help_style),
                    Span::styled("Tab", key_style),
                    Span::styled(" inspect | ", help_style),
                    Span::styled("Ctrl+*", key_style),
                    Span::styled(" history | ", help_style),
                    Span::styled("C", key_style),
                    Span::styled(" cache | ", help_style),
                    Span::styled("H", key_style),
                    Span::styled(" hide", help_style),
                ],
                PetriView::Inspector => vec![
                    Span::styled("Esc", key_style),
                    Span::styled(" close | ", help_style),
                    Span::styled("Space", key_style),
                    Span::styled(" pause | ", help_style),
                    Span::styled("Enter", key_style),
                    Span::styled(" step | ", help_style),
                    Span::styled("+/-", key_style),
                    Span::styled(" speed | ", help_style),
                    Span::styled("Tab", key_style),
                    Span::styled(" tournament | ", help_style),
                    Span::styled(
                        if session.snapshot.is_some() {
                            "←/→"
                        } else {
                            "C"
                        },
                        key_style,
                    ),
                    Span::styled(
                        if session.snapshot.is_some() {
                            " window | "
                        } else {
                            " cache | "
                        },
                        help_style,
                    ),
                    Span::styled("Ctrl+*", key_style),
                    Span::styled(" history | ", help_style),
                    if session.snapshot.is_some() {
                        Span::styled("C", key_style)
                    } else {
                        Span::styled("R", key_style)
                    },
                    if session.snapshot.is_some() {
                        Span::styled(" cache | ", help_style)
                    } else {
                        Span::styled(" refresh | ", help_style)
                    },
                    Span::styled("H", key_style),
                    Span::styled(" hide", help_style),
                ],
                PetriView::Cache => vec![
                    Span::styled("Esc", key_style),
                    Span::styled(" close | ", help_style),
                    Span::styled("Tab", key_style),
                    Span::styled(" tournament | ", help_style),
                    Span::styled("↑/↓", key_style),
                    Span::styled(" select | ", help_style),
                    Span::styled("R", key_style),
                    Span::styled(" refresh | ", help_style),
                    Span::styled("x", key_style),
                    Span::styled(" remove | ", help_style),
                    Span::styled("X", key_style),
                    Span::styled(" clear all | ", help_style),
                    Span::styled("H", key_style),
                    Span::styled(" hide", help_style),
                ],
            }));
        }

        state.games.petri_lines = lines_to_strings(&lines);
        let lines = apply_ui_selection(
            lines,
            state.ui_selection.as_ref(),
            UiSelectionPane::GamesPetriDish,
            theme.selection_bg,
            0,
        );
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(Block::default().style(Style::default().bg(theme.background)));
        frame.render_widget(paragraph, inner);
    }

    fn start_session(&mut self, state: &mut AppState) {
        state.games.family_building = false;
        if self.session.is_some() && state.games.running {
            self.warning = Some("Games tournament already running".into());
            return;
        }
        if self.session.is_some() && !state.games.running {
            self.session = None;
            self.hidden = false;
            self.warning = None;
        }
        let mut run_label: Option<String> = None;
        let mut family_mode = false;
        let (mut config, config_text) =
            if let Some(override_run) = state.games.pending_run_override.take() {
                run_label = Some(override_run.label);
                family_mode = override_run.family_mode;
                (override_run.config, override_run.config_text)
            } else {
                let config_text = state.editor_buffer().content_as_string();
                let version = state.editor_buffer().version();
                let Some(preview) = state
                    .games
                    .config_preview
                    .as_ref()
                    .filter(|preview| preview.version == version)
                else {
                    state.games.pending_run = true;
                    self.open_loading_popup(state, "Preparing run config in background...");
                    state.status = None;
                    return;
                };
                let config = match &preview.result {
                    Ok(config) => config.clone(),
                    Err(err) => {
                        let msg = format!("Config error: {err}");
                        state.games.status = GamesStatus::Error;
                        state.games.last_error = Some(msg.clone());
                        state.status = Some(msg);
                        return;
                    }
                };
                (config, config_text)
            };

        if family_mode {
            apply_family_run_runtime_overrides(&mut config);
        } else {
            config.engine.mode = nit_games::EngineMode::Interactive;
        }
        let timestamp = EventWriter::timestamp();
        let seed = config
            .seed
            .unwrap_or_else(|| stable_hash_bytes(format!("{timestamp}\n{config_text}").as_bytes()));
        config.seed = Some(seed);

        let run_id = run_id_from_seed_config(seed, &config_text);
        let layout = config
            .save_data
            .then(|| RunLayout::for_base(&state.workspace_root, &timestamp, seed, &run_id));
        if let Some(layout) = layout.as_ref() {
            if let Err(err) = fs::create_dir_all(&layout.run_dir) {
                let msg = format!("Failed to create games runs: {err}");
                state.games.status = GamesStatus::Error;
                state.games.last_error = Some(msg.clone());
                state.status = Some(msg);
                return;
            }
        }
        self.history_store = if family_mode || !config.save_data {
            None
        } else if let Some(layout) = layout.as_ref() {
            match HistoryPreviewStore::create(layout.run_dir.join("match_history_preview.ndjson")) {
                Ok(store) => Some(store),
                Err(err) => {
                    tracing::warn!("Failed to create match history preview cache: {err}");
                    state.games.match_history.last_error =
                        Some(format!("History preview cache disabled: {err}"));
                    None
                }
            }
        } else {
            None
        };

        let event_log_enabled = config.save_data && config.event_log.enabled;
        let history_log_enabled = config.save_data && config.history.enabled;
        let metal_path = config.engine.accelerator.allows_metal();
        let use_previews =
            !matches!(config.engine.mode, nit_games::EngineMode::Batch) && !metal_path;
        let use_event_log = event_log_enabled && !metal_path;
        let use_history_log = history_log_enabled && !metal_path;
        let steps_use_match_units =
            uses_match_step_units(&config, use_event_log, use_history_log, use_previews);
        let mut request_steps_per_tick = state.games.steps_per_tick.max(1);
        if family_mode {
            let strategy_count = config.strategies.len() as u32;
            let fast_family = config
                .strategies
                .iter()
                .all(|spec| FastStrategyModel::from_spec(spec).is_some());
            let turbo_matches = strategy_count
                .saturating_mul(strategy_count)
                .saturating_div(8)
                .clamp(256, if fast_family { 250_000 } else { 4_096 });
            let turbo_target = if steps_use_match_units {
                turbo_matches
            } else {
                turbo_matches.saturating_mul(config.rounds.max(1))
            };
            if request_steps_per_tick < turbo_target {
                request_steps_per_tick = turbo_target;
                state.games.steps_per_tick = turbo_target;
            }
        }
        let summary_path = layout.as_ref().map(|layout| layout.summary_path.clone());
        let event_path = layout.as_ref().map(|layout| layout.events_path.clone());
        let history_path = layout.as_ref().map(|layout| layout.history_path.clone());
        if let Some(summary_path) = summary_path.as_ref() {
            info!("Games summary path: {}", summary_path.display());
        }
        if let Some(event_path) = event_path.as_ref().filter(|_| event_log_enabled) {
            info!("Games event log path: {}", event_path.display());
        }
        if let Some(history_path) = history_path.as_ref().filter(|_| history_log_enabled) {
            info!("Games history log path: {}", history_path.display());
        }
        let progress_interval =
            std::time::Duration::from_millis(config.engine.progress_interval_ms);
        let requested_accelerator = config.engine.accelerator;
        let request = RunRequest {
            config: config.clone(),
            config_text: config_text.clone(),
            timestamp: timestamp.clone(),
            run_id: run_id.clone(),
            run_dir: layout.as_ref().map(|layout| layout.run_dir.clone()),
            summary_path: summary_path.clone(),
            definitions_path: layout
                .as_ref()
                .map(|layout| layout.definitions_path.clone()),
            results_path: layout.as_ref().map(|layout| layout.results_path.clone()),
            config_path: layout.as_ref().map(|layout| layout.config_path.clone()),
            analysis_dir: layout.as_ref().map(|layout| layout.analysis_dir.clone()),
            event_path: if event_log_enabled { event_path } else { None },
            history_path: if history_log_enabled {
                history_path
            } else {
                None
            },
            progress_interval,
            steps_per_tick: request_steps_per_tick,
        };

        self.runner.send(RunnerCommand::StartRun(Box::new(request)));
        self.loading_open = true;
        self.loading_message = Some("Starting tournament runner...".into());
        self.session = Some(GameSession {
            config,
            progress: None,
            snapshot: None,
            results: TournamentResults::empty(),
            definitions: Vec::new(),
            started_at: Instant::now(),
            finished_elapsed: None,
        });
        self.hidden = false;
        state.games.match_history.open = false;
        state.games.match_history.last_error = None;
        state.games.match_history.entries.clear();
        state.games.match_history.total_entries = 0;
        state.games.match_history.loaded_start = 0;
        state.games.match_history.max_rounds_seen = 0;
        state.games.match_history.column_offset = 0;
        state.games.match_history.round_limit = None;
        state.games.match_history.capture_disabled_for_run = family_mode;
        state.games.run_browser.open = false;
        state.games.replay.open = false;
        state.games.strategy_inspect.open = false;
        state.games.tm_sim.open = false;
        state.games.ca_sim.open = false;
        state.games.analysis.open = false;
        state.games.petri_hidden = false;
        state.games.running = true;
        state.games.paused = false;
        state.games.steps_use_match_units = steps_use_match_units;
        state.games.status = GamesStatus::Running;
        state.games.last_error = None;
        state.games.runtime = nit_games::RuntimeAcceleratorStats::new(requested_accelerator);
        let speed_unit = if steps_use_match_units {
            "matches/tick"
        } else {
            "steps/tick"
        };
        state.status = Some(match run_label {
            Some(label) if family_mode => {
                format!(
                    "Games tournament started ({label}, turbo {request_steps_per_tick} {speed_unit})"
                )
            }
            Some(label) => format!("Games tournament started ({label})"),
            None => "Games tournament started".into(),
        });
        info!("Games tournament started");
    }

    fn refresh_history_window(&mut self, state: &mut AppState) {
        let observed_total = if let Some(store) = self.history_store.as_ref() {
            store.len()
        } else {
            state.games.match_history.entries.len()
        };
        let total = state.games.match_history.total_entries.max(observed_total);
        state.games.match_history.total_entries = total;

        if total == 0 {
            state.games.match_history.loaded_start = 0;
            return;
        }

        if self.history_store.is_none() {
            state.games.match_history.loaded_start = 0;
            state.games.match_history.total_entries = observed_total;
            return;
        }

        if !state.games.match_history.open {
            return;
        }

        let desired = state
            .games
            .match_history
            .column_offset
            .min(total.saturating_sub(1));
        let load_start = desired.saturating_sub(HISTORY_LOAD_PREFETCH);
        let load_end = load_start.saturating_add(HISTORY_LOAD_CHUNK).min(total);
        let loaded_start = state.games.match_history.loaded_start;
        let loaded_end = loaded_start
            .saturating_add(state.games.match_history.entries.len())
            .min(total);
        let has_window = !state.games.match_history.entries.is_empty();
        let needs_reload = !has_window || desired < loaded_start || desired >= loaded_end;
        if !needs_reload {
            return;
        }

        if let Some(store) = self.history_store.as_mut() {
            match store.load_range(load_start, load_end.saturating_sub(load_start)) {
                Ok(entries) => {
                    state.games.match_history.entries = entries;
                    state.games.match_history.loaded_start = load_start;
                }
                Err(err) => {
                    state.games.match_history.last_error =
                        Some(format!("Failed to load history slice: {err}"));
                }
            }
        }
    }

    fn start_analysis(&mut self, state: &mut AppState, request: GamesAnalysisRequest) {
        if state.games.analysis.running {
            state.status = Some("Games analysis already running".into());
            return;
        }
        let raw_path = if let Some(path) = request.path {
            path
        } else if let Some(path) = state.games.last_history_path.clone() {
            path
        } else {
            state.status = Some("No history log available to analyze".into());
            state.games.analysis.running = false;
            return;
        };
        let cleaned = normalize_path(&raw_path);
        if cleaned.is_empty() {
            state.status = Some("No history log available to analyze".into());
            state.games.analysis.running = false;
            return;
        }
        let mut history_path = std::path::PathBuf::from(&cleaned);
        if history_path.is_relative() {
            history_path = state.workspace_root.join(history_path);
        }
        if !history_path.exists() {
            let fallback = state
                .workspace_root
                .join("runs")
                .join("games")
                .join(&cleaned);
            if fallback.exists() {
                history_path = fallback;
            }
        }
        if !history_path.exists() {
            let fallback = state.workspace_root.join("games-runs").join(&cleaned);
            if fallback.exists() {
                history_path = fallback;
            }
        }
        if !history_path.exists() {
            state.status = Some(format!("History log not found: {}", history_path.display()));
            state.games.analysis.running = false;
            return;
        }
        let out_dir = history_path
            .parent()
            .map(|p| p.join("analysis"))
            .unwrap_or_else(|| state.workspace_root.join("runs").join("games"));

        state.games.analysis.open = true;
        state.games.analysis.running = true;
        state.games.analysis.last_error = None;
        state.games.analysis.summary = None;
        state.games.analysis.preview = None;
        state.games.analysis.source_path = Some(history_path.display().to_string());

        let request = AnalysisRequest {
            history_path,
            out_dir,
            tail_rounds: request.tail_rounds,
            trajectory_samples: request.trajectory_samples,
        };
        self.analysis_runner.send(AnalysisCommand::Analyze(request));
    }

    fn close(&mut self, state: &mut AppState) {
        if self.session.is_some() {
            self.runner.send(RunnerCommand::Cancel);
        }
        self.family_run_rx = None;
        self.session = None;
        self.loading_open = false;
        self.loading_message = None;
        self.hidden = false;
        state.games.running = false;
        state.games.paused = false;
        state.games.steps_use_match_units = false;
        state.games.family_building = false;
        state.games.pending_run = false;
        state.games.pending_run_override = None;
        state.games.pending_family_run = None;
        state.games.status = GamesStatus::Idle;
        state.games.petri_hidden = false;
        state.games.petri_lines.clear();
        if let Some(selection) = state.ui_selection {
            if matches!(selection.pane, UiSelectionPane::GamesPetriDish) {
                state.ui_selection = None;
            }
        }
    }

    fn hide(&mut self, state: &mut AppState) {
        if self.session.is_some() || self.loading_open {
            self.hidden = true;
            state.games.petri_hidden = true;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesPetriDish) {
                    state.ui_selection = None;
                }
            }
        }
    }

    fn show(&mut self, state: &mut AppState) {
        if self.session.is_some() || self.loading_open {
            self.hidden = false;
            state.games.petri_hidden = false;
        }
    }

    fn open_loading_popup(&mut self, state: &mut AppState, message: impl Into<String>) {
        self.loading_open = true;
        self.loading_message = Some(message.into());
        self.hidden = false;
        state.games.running = true;
        state.games.paused = false;
        state.games.status = GamesStatus::Running;
        state.games.petri_hidden = false;
        state.games.last_error = None;
    }

    fn adjust_inspector_window(&mut self, delta: i32) {
        const WINDOWS: [usize; 4] = [20, 50, 100, 200];
        let current = self.inspector_window;
        let mut idx = WINDOWS.iter().position(|v| *v == current).unwrap_or(1);
        if delta.is_positive() {
            idx = (idx + 1).min(WINDOWS.len() - 1);
        } else if delta.is_negative() {
            idx = idx.saturating_sub(1);
        }
        self.inspector_window = WINDOWS[idx];
    }

    fn export_last_run(&mut self, state: &mut AppState) {
        let Some(run) = state.games.last_run.as_ref() else {
            state.status = Some("No completed run to export".into());
            return;
        };
        let timestamp = EventWriter::timestamp();
        let layout = RunLayout::for_base(&state.workspace_root, &timestamp, run.seed, &run.run_id);
        if let Err(err) = fs::create_dir_all(&layout.run_dir) {
            state.status = Some(format!("Failed to create games runs: {err}"));
            return;
        }
        let summary_path = layout.summary_path.clone();
        let mut export_run = run.clone();
        export_run.paths.summary = Some(summary_path.display().to_string());
        export_run.paths.definitions = Some(layout.definitions_path.display().to_string());
        export_run.paths.results = Some(layout.results_path.display().to_string());
        export_run.paths.config = Some(layout.config_path.display().to_string());
        export_run.paths.analysis_dir = Some(layout.analysis_dir.display().to_string());
        export_run.run_dir = Some(layout.run_dir.display().to_string());
        export_run.event_log = export_run.paths.events.clone();
        export_run.history_log = export_run.paths.history.clone();

        if let Err(err) = std::fs::write(&layout.config_path, &export_run.config_text) {
            tracing::warn!("Failed to write games config snapshot: {err}");
        }
        if let Err(err) = nit_utils::fs::write_atomic(&layout.definitions_path, |writer| {
            serde_json::to_writer_pretty(writer, &export_run.strategies)
                .map_err(std::io::Error::other)
        }) {
            tracing::warn!("Failed to write games definitions: {err}");
        }
        if let Err(err) = nit_utils::fs::write_atomic(&layout.results_path, |writer| {
            serde_json::to_writer_pretty(writer, &export_run.results).map_err(std::io::Error::other)
        }) {
            tracing::warn!("Failed to write games results: {err}");
        }
        if let Err(err) = nit_games::output::write_summary(&summary_path, &export_run) {
            state.status = Some(format!("Failed to export summary: {err}"));
        } else {
            state.games.last_run_path = Some(summary_path.display().to_string());
            state.status = Some(format!(
                "Games summary exported: {}",
                summary_path.display()
            ));
        }
    }
}

fn reset_strategy_inspect(state: &mut AppState) {
    state.games.strategy_inspect.last_error = None;
    state.games.strategy_inspect.title = None;
    state.games.strategy_inspect.lines.clear();
    state.games.strategy_inspect.definition = None;
    state.games.strategy_inspect.selected_index = 0;
    state.games.strategy_inspect.scroll_offset = 0;
    state.games.strategy_inspect.definitions.clear();
    state.games.strategy_inspect.source_label = None;
}

impl Drop for GamesPetriDishRuntime {
    fn drop(&mut self) {
        self.runner.shutdown();
        self.analysis_runner.shutdown();
        self.runs_runner.shutdown();
    }
}

fn normalize_path(input: &str) -> String {
    let trimmed = input.trim();
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

fn progress_waiting_text(status: GamesStatus) -> &'static str {
    match status {
        GamesStatus::Idle => "Waiting for tournament...",
        GamesStatus::Running => "Starting tournament...",
        GamesStatus::Paused => "Paused before first round",
        GamesStatus::Done => "Tournament complete",
        GamesStatus::Error => "Tournament unavailable",
    }
}

fn progress_pending_round_text(status: GamesStatus) -> &'static str {
    match status {
        GamesStatus::Running => "Round pending...",
        GamesStatus::Paused => "Paused",
        _ => progress_waiting_text(status),
    }
}

fn format_tournament_elapsed(duration: Duration) -> String {
    if duration.as_secs() == 0 {
        return format!("{}ms", duration.as_millis());
    }
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let hours = total_secs / 3_600;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}.{millis:03}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}.{millis:03}s")
    } else {
        format!("{seconds}.{millis:03}s")
    }
}

fn session_footer_line(
    steps_per_tick: u32,
    steps_use_match_units: bool,
    paused: bool,
    elapsed: Duration,
    label_style: Style,
    number_style: Style,
    paused_style: Style,
    dim_style: Style,
) -> Line<'static> {
    let speed_label = if steps_use_match_units {
        "matches/tick: "
    } else {
        "steps/tick: "
    };
    Line::from(vec![
        Span::styled(speed_label, label_style),
        Span::styled(steps_per_tick.to_string(), number_style),
        Span::styled("  ", dim_style),
        Span::styled("paused: ", label_style),
        Span::styled(if paused { "yes" } else { "no" }, paused_style),
        Span::styled("  ", dim_style),
        Span::styled("elapsed: ", label_style),
        Span::styled(format_tournament_elapsed(elapsed), number_style),
    ])
}

fn tournament_progress_percent(progress: &TournamentProgress) -> f32 {
    if progress.total_matches == 0 || progress.rounds == 0 {
        return 0.0;
    }
    let completed_matches = progress.match_index.saturating_sub(1) as u128;
    let round = progress.round.min(progress.rounds) as u128;
    let rounds_per_match = progress.rounds as u128;
    let total_rounds = (progress.total_matches as u128).saturating_mul(rounds_per_match);
    if total_rounds == 0 {
        return 0.0;
    }
    let done_rounds = completed_matches
        .saturating_mul(rounds_per_match)
        .saturating_add(round);
    ((done_rounds as f64 / total_rounds as f64) * 100.0) as f32
}

#[allow(clippy::too_many_arguments)]
fn render_progress(
    progress: Option<TournamentProgress>,
    definitions: &[StrategyDefinition],
    state: &AppState,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
    status_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let waiting_text = progress_waiting_text(state.games.status);
    let pending_round_text = progress_pending_round_text(state.games.status);
    lines.push(Line::from(vec![
        Span::styled("Status: ", label_style),
        Span::styled(format!("{:?}", state.games.status), status_style),
    ]));
    if let Some(progress) = progress {
        if progress.total_matches == 0 {
            lines.push(Line::from(vec![
                Span::styled("Match: ", label_style),
                Span::styled("0/0", number_style),
                Span::styled(" (no matches scheduled)", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Pair: ", label_style),
                Span::styled("n/a", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Last: ", label_style),
                Span::styled("n/a", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Halt: ", label_style),
                Span::styled("n/a", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Payoff: ", label_style),
                Span::styled("n/a", dim_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Total: ", label_style),
                Span::styled("0", number_style),
                Span::styled(" / ", dim_style),
                Span::styled("0", number_style),
            ]));
            lines.push(accelerator_progress_line(
                &progress.runtime,
                label_style,
                value_style,
                number_style,
                dim_style,
            ));
            if let Some(note) = accelerator_note_line(&progress.runtime, label_style, dim_style) {
                lines.push(note);
            }
            if let Some(cache) = accelerator_cache_line(&progress.runtime, label_style, dim_style) {
                lines.push(cache);
            }
            return lines;
        }
        let a_label = strategy_label_for_pair(&progress.a, definitions);
        let b_label = strategy_label_for_pair(&progress.b, definitions);
        let pct = tournament_progress_percent(&progress);
        let running_completed_snapshot =
            progress.match_complete && progress.match_index < progress.total_matches;
        let mut match_spans = vec![
            Span::styled("Match: ", label_style),
            Span::styled(progress.match_index.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(progress.total_matches.to_string(), number_style),
            Span::styled(" (", dim_style),
        ];
        if running_completed_snapshot {
            match_spans.push(Span::styled("last complete, overall ", dim_style));
        } else {
            match_spans.push(Span::styled("round ", dim_style));
            match_spans.push(Span::styled(progress.round.to_string(), number_style));
            match_spans.push(Span::styled("/", dim_style));
            match_spans.push(Span::styled(progress.rounds.to_string(), number_style));
            match_spans.push(Span::styled(", overall ", dim_style));
        }
        match_spans.push(Span::styled(format!("{pct:>5.1}%"), number_style));
        match_spans.push(Span::styled(")", dim_style));
        lines.push(Line::from(match_spans));
        if running_completed_snapshot {
            let live_copy = if uses_metal_batching(&progress.runtime) {
                "GPU batching; showing last completed match snapshot"
            } else {
                "Showing last completed match snapshot"
            };
            lines.push(Line::from(vec![
                Span::styled("Live: ", label_style),
                Span::styled(live_copy, dim_style),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(a_label, value_style),
            Span::styled(" vs ", dim_style),
            Span::styled(b_label, value_style),
        ]));
        lines.push(Line::from(
            match (progress.last_action_a, progress.last_action_b) {
                (Some(a), Some(b)) => vec![
                    Span::styled("Last: ", label_style),
                    Span::styled(a.as_char().to_string(), number_style),
                    Span::styled(" / ", dim_style),
                    Span::styled(b.as_char().to_string(), number_style),
                ],
                _ => vec![
                    Span::styled("Last: ", label_style),
                    Span::styled(pending_round_text, dim_style),
                ],
            },
        ));
        lines.push(Line::from(
            match (progress.last_halted_a, progress.last_halted_b) {
                (Some(a), Some(b)) => vec![
                    Span::styled("Halt: ", label_style),
                    Span::styled(if a { "1" } else { "0" }, number_style),
                    Span::styled(" / ", dim_style),
                    Span::styled(if b { "1" } else { "0" }, number_style),
                    Span::styled(" ", dim_style),
                    Span::styled("(1=halt, 0=timeout)", dim_style),
                ],
                _ => vec![
                    Span::styled("Halt: ", label_style),
                    Span::styled(pending_round_text, dim_style),
                ],
            },
        ));
        if let (Some(pa), Some(pb)) = (progress.last_payoff_a, progress.last_payoff_b) {
            lines.push(Line::from(vec![
                Span::styled("Payoff: ", label_style),
                Span::styled(pa.to_string(), number_style),
                Span::styled(" / ", dim_style),
                Span::styled(pb.to_string(), number_style),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("Payoff: ", label_style),
                Span::styled(pending_round_text, dim_style),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("Total: ", label_style),
            Span::styled(progress.total_payoff_a.to_string(), number_style),
            Span::styled(" / ", dim_style),
            Span::styled(progress.total_payoff_b.to_string(), number_style),
        ]));
        lines.push(accelerator_progress_line(
            &progress.runtime,
            label_style,
            value_style,
            number_style,
            dim_style,
        ));
        if let Some(note) = accelerator_note_line(&progress.runtime, label_style, dim_style) {
            lines.push(note);
        }
        if let Some(cache) = accelerator_cache_line(&progress.runtime, label_style, dim_style) {
            lines.push(cache);
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Last: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Halt: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Payoff: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Total: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(accelerator_progress_line(
            &state.games.runtime,
            label_style,
            value_style,
            number_style,
            dim_style,
        ));
        if let Some(note) = accelerator_note_line(&state.games.runtime, label_style, dim_style) {
            lines.push(note);
        }
        if let Some(cache) = accelerator_cache_line(&state.games.runtime, label_style, dim_style) {
            lines.push(cache);
        }
    }
    lines
}

fn accelerator_progress_line(
    runtime: &nit_games::RuntimeAcceleratorStats,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
) -> Line<'static> {
    let backend = match runtime.backend {
        nit_games::RuntimeAcceleratorBackend::Metal => "metal",
        nit_games::RuntimeAcceleratorBackend::Cpu => "cpu",
        nit_games::RuntimeAcceleratorBackend::None => match runtime.requested {
            nit_games::AcceleratorMode::Cpu => "cpu",
            nit_games::AcceleratorMode::Metal => "metal",
            nit_games::AcceleratorMode::Auto => "auto",
        },
    };
    let mut spans = vec![
        Span::styled("Accel: ", label_style),
        Span::styled(backend.to_ascii_uppercase(), value_style),
    ];
    if runtime.metal_matches > 0 {
        spans.push(Span::styled(" ", dim_style));
        spans.push(Span::styled(
            format!("gpu {}", runtime.metal_matches),
            number_style,
        ));
    }
    if runtime.cpu_matches > 0 {
        spans.push(Span::styled(" ", dim_style));
        spans.push(Span::styled(
            format!("cpu {}", runtime.cpu_matches),
            number_style,
        ));
    }
    if runtime.metal_fallbacks > 0 {
        spans.push(Span::styled(" ", dim_style));
        spans.push(Span::styled(
            format!("fallback {}", runtime.metal_fallbacks),
            dim_style,
        ));
    }
    if let (Some(batch), Some(inflight)) = (
        runtime.metal_matches_per_batch,
        runtime.metal_inflight_batches,
    ) {
        spans.push(Span::styled(" ", dim_style));
        let policy_label = runtime
            .metal_policy_source_label()
            .map(|source| format!("policy {}x{} {}", batch, inflight, source))
            .unwrap_or_else(|| format!("policy {}x{}", batch, inflight));
        spans.push(Span::styled(policy_label, dim_style));
    }
    Line::from(spans)
}

fn uses_metal_batching(runtime: &nit_games::RuntimeAcceleratorStats) -> bool {
    matches!(runtime.backend, nit_games::RuntimeAcceleratorBackend::Metal)
        || runtime.metal_matches > 0
}

fn accelerator_note_line(
    runtime: &nit_games::RuntimeAcceleratorStats,
    label_style: Style,
    dim_style: Style,
) -> Option<Line<'static>> {
    let reason = runtime.metal_fallback_reason.as_ref()?;
    Some(Line::from(vec![
        Span::styled("AccelNote: ", label_style),
        Span::styled(reason.clone(), dim_style),
    ]))
}

fn accelerator_cache_line(
    runtime: &nit_games::RuntimeAcceleratorStats,
    label_style: Style,
    dim_style: Style,
) -> Option<Line<'static>> {
    let value = runtime
        .metal_policy_cache_path
        .as_ref()
        .cloned()
        .or_else(|| runtime.metal_policy_cache_key.as_ref().cloned())?;
    Some(Line::from(vec![
        Span::styled("AccelCache: ", label_style),
        Span::styled(value, dim_style),
    ]))
}

fn render_cache_browser(
    snapshot: &BatchPolicyCacheSnapshot,
    selected: usize,
    width: usize,
    header_style: Style,
    label_style: Style,
    value_style: Style,
    dim_style: Style,
    key_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled("Metal Cache", header_style)));
    lines.push(Line::from(vec![
        Span::styled("root: ", label_style),
        Span::styled(
            truncate_text(
                &snapshot
                    .root
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_string()),
                width.saturating_sub(6),
            ),
            dim_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("entries: ", label_style),
        Span::styled(snapshot.entries.len().to_string(), value_style),
    ]));
    lines.push(Line::from(""));

    if snapshot.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "No Metal cache entries.",
            dim_style,
        )));
        return lines;
    }

    let selected = selected.min(snapshot.entries.len().saturating_sub(1));
    let visible = 8usize.min(snapshot.entries.len());
    let start = selected
        .saturating_sub(visible / 2)
        .min(snapshot.entries.len() - visible);
    let end = start + visible;
    let row_width = width.saturating_sub(4).max(16);
    for (idx, entry) in snapshot.entries[start..end].iter().enumerate() {
        let absolute_idx = start + idx;
        let marker = if absolute_idx == selected { ">" } else { " " };
        let marker_style = if absolute_idx == selected {
            key_style
        } else {
            dim_style
        };
        let summary = format!(
            "{:>2}. {} {}x{}",
            absolute_idx + 1,
            entry.key,
            entry.matches_per_batch,
            entry.inflight_batches
        );
        lines.push(Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(" ", dim_style),
            Span::styled(truncate_text(&summary, row_width), value_style),
        ]));
    }

    let selected_entry = &snapshot.entries[selected];
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("key: ", label_style),
        Span::styled(selected_entry.key.clone(), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("payload: ", label_style),
        Span::styled(
            truncate_text(&selected_entry.payload_signature, width.saturating_sub(9)),
            dim_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("path: ", label_style),
        Span::styled(
            truncate_text(&selected_entry.path, width.saturating_sub(6)),
            dim_style,
        ),
    ]));
    lines
}

#[allow(clippy::too_many_arguments)]
fn render_match_inspector(
    snapshot: Option<MatchSnapshot>,
    progress: Option<TournamentProgress>,
    definitions: &[StrategyDefinition],
    status: GamesStatus,
    window: usize,
    width: usize,
    header_style: Style,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
    warn_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let waiting_text = progress_waiting_text(status);
    let title = if snapshot.is_some() {
        "Live Match"
    } else if progress.as_ref().is_some_and(|progress| {
        progress.match_complete && progress.match_index < progress.total_matches
    }) {
        "Last Completed Match"
    } else {
        "Match Inspector"
    };
    lines.push(Line::from(Span::styled(title, header_style)));

    if let Some(snapshot) = snapshot {
        lines.push(Line::from(vec![
            Span::styled("window: ", label_style),
            Span::styled(window.to_string(), number_style),
            Span::styled(" rounds", dim_style),
        ]));
        let a_label = strategy_label_for_pair(&snapshot.a, definitions);
        let b_label = strategy_label_for_pair(&snapshot.b, definitions);
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(snapshot.match_index.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(snapshot.total_matches.to_string(), number_style),
            Span::styled(" (round ", dim_style),
            Span::styled(snapshot.round.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(snapshot.rounds.to_string(), number_style),
            Span::styled(")", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(a_label, value_style),
            Span::styled(" vs ", dim_style),
            Span::styled(b_label, value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Score: ", label_style),
            Span::styled(snapshot.a_score.to_string(), number_style),
            Span::styled(" / ", dim_style),
            Span::styled(snapshot.b_score.to_string(), number_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Outcomes: ", label_style),
            Span::styled("0=CC 1=CD 2=DC 3=DD", dim_style),
        ]));
        lines.extend(render_match_strip(
            &snapshot,
            window,
            width,
            label_style,
            value_style,
            number_style,
            dim_style,
            warn_style,
        ));
    } else if let Some(progress) = progress {
        let a_label = strategy_label_for_pair(&progress.a, definitions);
        let b_label = strategy_label_for_pair(&progress.b, definitions);
        let running_completed_snapshot =
            progress.match_complete && progress.match_index < progress.total_matches;
        let match_detail = if running_completed_snapshot {
            "last complete".to_string()
        } else if progress.round > 0 {
            format!("round {}/{}", progress.round, progress.rounds)
        } else {
            waiting_text.to_string()
        };
        let last_detail = match (
            progress.last_action_a,
            progress.last_action_b,
            progress.last_payoff_a,
            progress.last_payoff_b,
        ) {
            (Some(a), Some(b), Some(payoff_a), Some(payoff_b)) => {
                format!(
                    "{} / {} ({payoff_a} / {payoff_b})",
                    a.as_char(),
                    b.as_char()
                )
            }
            _ => waiting_text.to_string(),
        };
        let halt_detail = match (progress.last_halted_a, progress.last_halted_b) {
            (Some(a), Some(b)) => format!("{} / {} (1=halt, 0=timeout)", a as u8, b as u8),
            _ => waiting_text.to_string(),
        };
        let halt_waiting = halt_detail == waiting_text;
        let note = if running_completed_snapshot {
            if uses_metal_batching(&progress.runtime) {
                "Detailed round history is unavailable during GPU batching; showing the last completed match summary."
            } else {
                "Detailed round history is unavailable; showing the last completed match summary."
            }
        } else {
            "Waiting for a live per-round match snapshot."
        };
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(progress.match_index.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(progress.total_matches.to_string(), number_style),
            Span::styled(" (", dim_style),
            Span::styled(match_detail, dim_style),
            Span::styled(")", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(a_label, value_style),
            Span::styled(" vs ", dim_style),
            Span::styled(b_label, value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Total: ", label_style),
            Span::styled(progress.total_payoff_a.to_string(), number_style),
            Span::styled(" / ", dim_style),
            Span::styled(progress.total_payoff_b.to_string(), number_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Last: ", label_style),
            Span::styled(last_detail, value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Halt: ", label_style),
            Span::styled(
                halt_detail,
                if halt_waiting { dim_style } else { warn_style },
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Note: ", label_style),
            Span::styled(truncate_text(note, width.saturating_sub(6)), dim_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Score: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Outcomes: ", label_style),
            Span::styled(waiting_text, dim_style),
        ]));
    }
    lines
}

fn strategy_label_for_pair(id: &str, definitions: &[StrategyDefinition]) -> String {
    let Some(def) = definitions.iter().find(|def| def.id == id) else {
        return id.to_string();
    };
    match &def.kind {
        nit_games::config::StrategySpecKind::Fsm {
            num_states,
            outputs,
            transitions,
            index,
            ..
        } => {
            let states = if outputs.is_empty() {
                *num_states
            } else {
                outputs.len()
            };
            let k = transitions.first().map(|row| row.len()).unwrap_or(2);
            if let Some(index) = index {
                format!("{id} {{n={index}, s={states}, k={k}}}")
            } else {
                format!("{id} {{s={states}, k={k}}}")
            }
        }
        nit_games::config::StrategySpecKind::Ca { n, k, r, t } => {
            let _ = t;
            format!("{id} {{n={n}, k={k}, r={r}}}")
        }
        nit_games::config::StrategySpecKind::OneSidedTm {
            rule_code,
            states,
            symbols,
            ..
        } => {
            if let Some(rule) = rule_code {
                format!("{id} {{n={rule}, s={states}, k={symbols}}}")
            } else {
                format!("{id} {{s={states}, k={symbols}}}")
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_match_strip(
    snapshot: &MatchSnapshot,
    window: usize,
    width: usize,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
    warn_style: Style,
) -> Vec<Line<'static>> {
    let total = snapshot.outcomes.len().min(snapshot.payoffs.len());
    if total == 0 || window == 0 {
        return vec![Line::from(vec![
            Span::styled("  ", dim_style),
            Span::styled("--", dim_style),
        ])];
    }
    let mut cumulative = Vec::with_capacity(total);
    let mut a_total = 0i64;
    let mut b_total = 0i64;
    for payoff in snapshot.payoffs.iter().take(total) {
        a_total += payoff[0] as i64;
        b_total += payoff[1] as i64;
        cumulative.push((a_total, b_total));
    }

    let halt_token = |index: usize| -> String {
        let a = snapshot.a_halted.as_bytes().get(index).copied();
        let b = snapshot.b_halted.as_bytes().get(index).copied();
        match (a, b) {
            (Some(a), Some(b)) => format!("{}/{}", a as char, b as char),
            _ => "--".to_string(),
        }
    };

    let label_w = 3usize;
    let prefix_len = label_w + 2;
    let available = width.saturating_sub(prefix_len);
    let mut max_len = 3usize;
    let window_start = total.saturating_sub(window);
    for (i, ((&idx_byte, payoff), cumulative)) in snapshot
        .outcomes
        .as_bytes()
        .iter()
        .take(total)
        .zip(snapshot.payoffs.iter().take(total))
        .zip(cumulative.iter())
        .enumerate()
        .skip(window_start)
    {
        let round_len = (i + 1).to_string().chars().count();
        let idx_char = idx_byte as char;
        let out_len = match idx_char {
            '0' | '1' | '2' | '3' => 3,
            _ => 2,
        };
        let payoff_len = format!("{}/{}", payoff[0], payoff[1]).chars().count();
        let total_len = format!("{}/{}", cumulative.0, cumulative.1).chars().count();
        let halt_len = halt_token(i).chars().count();
        max_len = max_len
            .max(round_len)
            .max(out_len)
            .max(payoff_len)
            .max(total_len)
            .max(halt_len);
    }
    let col_w = (max_len + 1).max(4);
    let max_cols = (available / col_w).max(1);
    let visible = window.min(total).min(max_cols);
    let start = total.saturating_sub(visible);

    let fit_right = |value: &str| -> String {
        if col_w == 0 {
            return String::new();
        }
        let len = value.chars().count();
        let trimmed: String = if len > col_w - 1 {
            value.chars().skip(len.saturating_sub(col_w - 1)).collect()
        } else {
            value.to_string()
        };
        format!("{:>width$} ", trimmed, width = col_w - 1)
    };
    let mut idx_line = String::new();
    let mut out_spans = Vec::new();
    let mut halt_spans = Vec::new();
    let mut pay_line = String::new();
    let mut total_line = String::new();
    out_spans.push(Span::styled(
        format!("{:>label_w$}: ", "Out", label_w = label_w),
        label_style,
    ));
    halt_spans.push(Span::styled(
        format!("{:>label_w$}: ", "Hlt", label_w = label_w),
        label_style,
    ));
    for (i, ((&idx_byte, payoff), cumulative)) in snapshot
        .outcomes
        .as_bytes()
        .iter()
        .take(total)
        .zip(snapshot.payoffs.iter().take(total))
        .zip(cumulative.iter())
        .enumerate()
        .skip(start)
    {
        idx_line.push_str(&fit_right(&(i + 1).to_string()));
        let idx_char = idx_byte as char;
        let outcome = match idx_char {
            '0' => "C/C",
            '1' => "C/D",
            '2' => "D/C",
            '3' => "D/D",
            _ => "--",
        };
        let outcome_style = match idx_char {
            '0' => number_style,
            '1' => value_style,
            '2' => warn_style,
            '3' => dim_style,
            _ => dim_style,
        };
        out_spans.push(Span::styled(fit_right(outcome), outcome_style));
        let halt = halt_token(i);
        let halt_style = match (
            snapshot.a_halted.as_bytes().get(i).copied(),
            snapshot.b_halted.as_bytes().get(i).copied(),
        ) {
            (Some(b'1'), Some(b'1')) => dim_style,
            (Some(_), Some(_)) => warn_style,
            _ => dim_style,
        };
        halt_spans.push(Span::styled(fit_right(&halt), halt_style));
        pay_line.push_str(&fit_right(&format!("{}/{}", payoff[0], payoff[1])));
        total_line.push_str(&fit_right(&format!("{}/{}", cumulative.0, cumulative.1)));
    }

    let separator = "-".repeat(width.min(prefix_len + visible * col_w));
    let legend = Line::from(vec![
        Span::styled("Legend: ", label_style),
        Span::styled("CC", number_style),
        Span::styled(" ", dim_style),
        Span::styled("CD", value_style),
        Span::styled(" ", dim_style),
        Span::styled("DC", warn_style),
        Span::styled(" ", dim_style),
        Span::styled("DD", dim_style),
        Span::styled(" | Hlt: 1=halt 0=timeout", dim_style),
    ]);

    vec![
        legend,
        Line::from(vec![
            Span::styled(
                format!("{:>label_w$}: ", "Idx", label_w = label_w),
                label_style,
            ),
            Span::styled(idx_line, number_style),
        ]),
        Line::from(out_spans),
        Line::from(halt_spans),
        Line::from(vec![
            Span::styled(
                format!("{:>label_w$}: ", "Pay", label_w = label_w),
                label_style,
            ),
            Span::styled(pay_line, number_style),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{:>label_w$}: ", "Tot", label_w = label_w),
                label_style,
            ),
            Span::styled(total_line, number_style),
        ]),
        Line::from(Span::styled(separator, dim_style)),
    ]
}

fn status_style(state: &AppState, theme: &Theme) -> Style {
    match state.games.status {
        GamesStatus::Idle => Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
        GamesStatus::Running => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        GamesStatus::Paused => Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD),
        GamesStatus::Done => Style::default().fg(theme.title),
        GamesStatus::Error => Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
    }
}

fn lines_to_strings(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn top_table_widths(config: &nit_games::NormalizedConfig) -> (usize, usize, usize, usize) {
    let n = config.strategies.len().max(1);
    let matches_per = if config.self_play {
        n.saturating_mul(2)
    } else {
        n.saturating_sub(1).saturating_mul(2)
    };
    let matches_per = matches_per.saturating_mul(config.repetitions.max(1) as usize);
    let rounds = config.rounds.max(1) as i64;
    let mut max_payoff = i32::MIN;
    let mut min_payoff = i32::MAX;
    for row in config.payoff.matrix.iter() {
        for cell in row.iter() {
            for value in cell.iter() {
                max_payoff = max_payoff.max(*value);
                min_payoff = min_payoff.min(*value);
            }
        }
    }
    let max_payoff = max_payoff as i64;
    let min_payoff = min_payoff as i64;
    let max_abs = max_payoff.abs().max(min_payoff.abs());
    let score_header = score_column_label(config);
    let total_header = total_payoff_column_label(config);
    let score_bound = match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => max_abs as f64,
        nit_games::ScoreAggregation::Total => max_abs
            .saturating_mul(matches_per as i64)
            .saturating_mul(rounds) as f64,
    };
    let total_bound = match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => max_abs.saturating_mul(matches_per as i64) as f64,
        nit_games::ScoreAggregation::Total => max_abs
            .saturating_mul(matches_per as i64)
            .saturating_mul(rounds) as f64,
    };
    let score_w = if min_payoff < 0 {
        nit_games::output::format_score_value(-score_bound).len()
    } else {
        nit_games::output::format_score_value(score_bound).len()
    }
    .max(score_header.len());
    let total_w = if min_payoff < 0 {
        nit_games::output::format_score_value(-total_bound).len()
    } else {
        nit_games::output::format_score_value(total_bound).len()
    }
    .max(total_header.len());
    let rank_w = n.to_string().len();
    let wld_w = format!("W{matches_per}-L{matches_per}-D{matches_per}").len();
    (rank_w, score_w, total_w, wld_w)
}

#[allow(clippy::too_many_arguments)]
fn render_top_table(
    results: &nit_games::output::TournamentResults,
    config: &nit_games::NormalizedConfig,
    definitions: &[nit_games::output::StrategyDefinition],
    width: usize,
    header_style: Style,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    win_style: Style,
    loss_style: Style,
    draw_style: Style,
    dim_style: Style,
    fixed_rank_w: usize,
    fixed_score_w: usize,
    fixed_total_w: usize,
    fixed_wld_w: usize,
) -> Vec<Line<'static>> {
    const TOP_LIMIT: usize = 15;
    type Row = (
        String,
        String,
        String,
        String,
        String,
        String,
        u32,
        u32,
        u32,
    );
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled("Top Strategies", header_style)));
    if definitions.is_empty() {
        lines.push(Line::from(Span::styled(
            "Loading strategy definitions...",
            dim_style,
        )));
        return lines;
    }
    if results.ranking.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for leaderboard results...",
            dim_style,
        )));
        return lines;
    }

    let score_header = score_column_label(config);
    let total_header = total_payoff_column_label(config);
    let rows: Vec<Row> = results
        .ranking
        .iter()
        .take(TOP_LIMIT)
        .enumerate()
        .map(|(idx, entry)| {
            let found = definitions.iter().find(|def| def.id == entry.id).cloned();
            let display = found
                .as_ref()
                .map(strategy_display_name_from_def)
                .unwrap_or_else(|| entry.id.clone());
            let machine_n = found
                .as_ref()
                .and_then(strategy_machine_index)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let rank = format!("{}", idx + 1);
            let id = entry.id.clone();
            let score = entry.formatted_score(
                config.engine.score_aggregation,
                config.engine.complexity_cost.enabled,
            );
            let total = entry.formatted_total_payoff(
                config.engine.score_aggregation,
                config.engine.complexity_cost.enabled,
            );
            (
                rank,
                id,
                machine_n,
                display,
                score,
                total,
                entry.wins,
                entry.losses,
                entry.draws,
            )
        })
        .collect();

    let headers = [
        "#",
        "id",
        "n",
        "Strategy",
        score_header,
        total_header,
        "W-L-D",
    ];
    let mut rank_w = headers[0].len().max(fixed_rank_w);
    let mut id_w = headers[1].len();
    let mut n_w = headers[2].len();
    let mut name_w = headers[3].len();
    let mut score_w = headers[4].len().max(fixed_score_w);
    let mut total_w = headers[5].len().max(fixed_total_w);
    let mut wld_w = headers[6].len().max(fixed_wld_w);

    for (rank, id, machine_n, name, score, total, wins, losses, draws) in &rows {
        rank_w = rank_w.max(rank.len());
        id_w = id_w.max(id.len());
        n_w = n_w.max(machine_n.len());
        name_w = name_w.max(name.chars().count());
        score_w = score_w.max(score.len());
        total_w = total_w.max(total.len());
        let wld_len = format!("W{wins}-L{losses}-D{draws}").len();
        wld_w = wld_w.max(wld_len);
    }

    let min_id = 4usize;
    let min_name = 10usize;
    let columns = headers.len();
    let overhead = (columns + 1) + (2 * columns);
    let fixed = rank_w + n_w + score_w + total_w + wld_w;
    let available = width.saturating_sub(overhead + fixed);
    if available >= min_id + min_name {
        id_w = id_w.min(available.saturating_sub(min_name));
        name_w = name_w.min(available.saturating_sub(id_w));
    } else {
        id_w = id_w.min(available.saturating_sub(1).max(1));
        name_w = available.saturating_sub(id_w).max(1);
    }

    let sep = format!(
        "+{}+{}+{}+{}+{}+{}+{}+",
        "-".repeat(rank_w + 2),
        "-".repeat(id_w + 2),
        "-".repeat(n_w + 2),
        "-".repeat(name_w + 2),
        "-".repeat(score_w + 2),
        "-".repeat(total_w + 2),
        "-".repeat(wld_w + 2)
    );
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));
    lines.push(Line::from(vec![
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[0], rank_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(format!(" {} ", center_text(headers[1], id_w)), header_style),
        Span::styled("|", dim_style),
        Span::styled(format!(" {} ", center_text(headers[2], n_w)), header_style),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[3], name_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[4], score_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[5], total_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[6], wld_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
    ]));
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));

    for (rank, id, machine_n, name, score, total, wins, losses, draws) in rows {
        let mut spans = Vec::new();
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {rank:>rank_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:<id_w$} ", truncate_text(&id, id_w)),
            label_style,
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:>n_w$} ", truncate_text(&machine_n, n_w)),
            if machine_n == "-" {
                dim_style
            } else {
                number_style
            },
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:<name_w$} ", truncate_text(&name, name_w)),
            value_style,
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {score:>score_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {total:>total_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        spans.extend(wld_cell_spans(
            wins,
            losses,
            draws,
            wld_w,
            label_style,
            win_style,
            loss_style,
            draw_style,
            dim_style,
        ));
        spans.push(Span::styled("|", dim_style));
        lines.push(Line::from(spans));
    }

    if results.ranking.len() > TOP_LIMIT {
        lines.push(Line::from(vec![
            Span::styled("… showing top ", dim_style),
            Span::styled(TOP_LIMIT.to_string(), number_style),
            Span::styled(" of ", dim_style),
            Span::styled(results.ranking.len().to_string(), number_style),
            Span::styled(" strategies", dim_style),
        ]));
    }

    lines.push(Line::from(Span::styled(sep, dim_style)));
    lines
}

fn score_column_label(config: &nit_games::NormalizedConfig) -> &'static str {
    match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => "Score(mean)",
        nit_games::ScoreAggregation::Total => "Score(total)",
    }
}

fn total_payoff_column_label(config: &nit_games::NormalizedConfig) -> &'static str {
    match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => "AggPayoff",
        nit_games::ScoreAggregation::Total => "TotalPayoff",
    }
}

fn strategy_machine_index(def: &StrategyDefinition) -> Option<u64> {
    match &def.kind {
        nit_games::config::StrategySpecKind::Fsm { index, .. } => *index,
        nit_games::config::StrategySpecKind::Ca { n, .. } => Some(*n),
        nit_games::config::StrategySpecKind::OneSidedTm { rule_code, .. } => *rule_code,
    }
}

#[allow(clippy::too_many_arguments)]
fn wld_cell_spans(
    wins: u32,
    losses: u32,
    draws: u32,
    width: usize,
    label_style: Style,
    win_style: Style,
    loss_style: Style,
    draw_style: Style,
    dim_style: Style,
) -> Vec<Span<'static>> {
    let base = format!("W{wins}-L{losses}-D{draws}");
    let pad = width.saturating_sub(base.len());
    let mut spans = Vec::new();
    spans.push(Span::styled(" ", dim_style));
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), dim_style));
    }
    spans.push(Span::styled("W", label_style));
    spans.push(Span::styled(wins.to_string(), win_style));
    spans.push(Span::styled("-L", label_style));
    spans.push(Span::styled(losses.to_string(), loss_style));
    spans.push(Span::styled("-D", label_style));
    spans.push(Span::styled(draws.to_string(), draw_style));
    spans.push(Span::styled(" ", dim_style));
    spans
}

fn center_text(value: &str, width: usize) -> String {
    let len = value.chars().count();
    if len >= width {
        return truncate_text(value, width);
    }
    let pad = width - len;
    let left = pad / 2;
    let right = pad - left;
    format!("{}{}{}", " ".repeat(left), value, " ".repeat(right))
}

fn truncate_text(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let len = value.chars().count();
    if len <= width {
        return value.to_string();
    }
    if width <= 3 {
        return value.chars().take(width).collect();
    }
    let mut out: String = value.chars().take(width - 3).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{decrease_steps_per_tick, increase_steps_per_tick};

    fn sample_snapshot() -> MatchSnapshot {
        MatchSnapshot {
            match_index: 1,
            total_matches: 1,
            round: 3,
            rounds: 3,
            a: "a".into(),
            b: "b".into(),
            a_score: -4,
            b_score: -2,
            outcomes: "013".into(),
            payoffs: vec![[-1, -1], [-3, 0], [0, -1]],
            a_halted: "110".into(),
            b_halted: "111".into(),
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn sample_config() -> nit_games::NormalizedConfig {
        nit_games::GamesConfig::from_toml(
            r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#,
        )
        .expect("parse config")
    }

    fn cache_test_runtime() -> (GamesPetriDishRuntime, AppState) {
        let mut state = AppState::new(
            std::env::temp_dir(),
            nit_core::Buffer::empty("x", None),
            nit_core::Buffer::empty("n", None),
        );
        let mut runtime = GamesPetriDishRuntime::new(&state);
        runtime.session = Some(GameSession {
            config: sample_config(),
            progress: None,
            snapshot: None,
            results: TournamentResults::empty(),
            definitions: Vec::new(),
            started_at: Instant::now(),
            finished_elapsed: None,
        });
        runtime.view = PetriView::Cache;
        runtime.cache_snapshot = nit_metal::BatchPolicyCacheSnapshot {
            root: Some("/tmp/metal-policy".into()),
            entries: vec![nit_metal::BatchPolicyCacheEntryInfo {
                key: "apple_m4_max_a".into(),
                path: "/tmp/metal-policy/apple_m4_max_a_v1.json".into(),
                device_name: "Apple M4 Max".into(),
                payload_signature: "fsm_s4_a2_n51924_static1mib".into(),
                matches_per_batch: 262_144,
                inflight_batches: 4,
            }],
        };
        state.games.status = GamesStatus::Running;
        (runtime, state)
    }

    #[test]
    fn render_cache_browser_shows_selected_entry_details() {
        let snapshot = nit_metal::BatchPolicyCacheSnapshot {
            root: Some("/tmp/metal-policy".into()),
            entries: vec![
                nit_metal::BatchPolicyCacheEntryInfo {
                    key: "apple_m4_max_a".into(),
                    path: "/tmp/metal-policy/apple_m4_max_a_v1.json".into(),
                    device_name: "Apple M4 Max".into(),
                    payload_signature: "fsm_s4_a2_n51924_static1mib".into(),
                    matches_per_batch: 262_144,
                    inflight_batches: 4,
                },
                nit_metal::BatchPolicyCacheEntryInfo {
                    key: "apple_m4_max_b".into(),
                    path: "/tmp/metal-policy/apple_m4_max_b_v1.json".into(),
                    device_name: "Apple M4 Max".into(),
                    payload_signature: "tm_s2_sym2_steps64_n128_static1mib".into(),
                    matches_per_batch: 32_768,
                    inflight_batches: 4,
                },
            ],
        };

        let lines = render_cache_browser(
            &snapshot,
            1,
            96,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );
        assert!(line_text(&lines[0]).contains("Metal Cache"));
        assert!(line_text(&lines[1]).contains("/tmp/metal-policy"));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("apple_m4_max_b 32768x4")));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("payload: tm_s2_sym2_steps64_n128_static1mib")));
        assert!(
            lines
                .iter()
                .any(|line| line_text(line)
                    .contains("path: /tmp/metal-policy/apple_m4_max_b_v1.json"))
        );
    }

    #[test]
    fn cache_clear_all_requires_confirmation() {
        let (mut runtime, mut state) = cache_test_runtime();
        assert!(runtime.handle_key(
            &KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
            &mut state
        ));
        assert!(runtime.confirm_clear_all_cache);
    }

    #[test]
    fn cache_clear_all_confirmation_can_be_cancelled() {
        let (mut runtime, mut state) = cache_test_runtime();
        runtime.confirm_clear_all_cache = true;
        assert!(runtime.handle_key(
            &KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut state
        ));
        assert!(!runtime.confirm_clear_all_cache);
        assert_eq!(state.status.as_deref(), Some("Metal cache clear cancelled"));
    }

    #[test]
    fn match_inspector_uses_progress_summary_when_snapshot_is_missing() {
        let mut runtime = nit_games::RuntimeAcceleratorStats::default();
        runtime.backend = nit_games::RuntimeAcceleratorBackend::Metal;
        runtime.metal_matches = 1024;
        let lines = render_match_inspector(
            None,
            Some(TournamentProgress {
                match_index: 345,
                total_matches: 1000,
                round: 200,
                rounds: 200,
                match_complete: true,
                a: "a".into(),
                b: "b".into(),
                total_payoff_a: 0,
                total_payoff_b: -600,
                last_action_a: Some(nit_games::Action::Defect),
                last_action_b: Some(nit_games::Action::Cooperate),
                last_payoff_a: Some(0),
                last_payoff_b: Some(-3),
                last_halted_a: Some(true),
                last_halted_b: Some(true),
                last_outcome: Some(nit_games::game::Outcome::DC),
                runtime,
            }),
            &[],
            GamesStatus::Running,
            50,
            120,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );
        assert!(line_text(&lines[0]).contains("Last Completed Match"));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("last complete")));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("Total: 0 / -600")));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("Last: D / C (0 / -3)")));
        assert!(lines.iter().any(|line| {
            line_text(line).contains("Detailed round history is unavailable during GPU batching")
        }));
    }

    #[test]
    fn match_inspector_uses_live_title_for_snapshot_mode() {
        let lines = render_match_inspector(
            Some(sample_snapshot()),
            None,
            &[],
            GamesStatus::Running,
            50,
            120,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert!(line_text(&lines[0]).contains("Live Match"));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("Match: 1/1 (round 3/3)")));
    }

    #[test]
    fn petri_rect_uses_taller_height_on_large_screens() {
        let rect = petri_rect(Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 48,
        });

        assert_eq!(rect.height, 40);
    }

    #[test]
    fn match_strip_renders_halt_row_and_timeout_markers() {
        let lines = render_match_strip(
            &sample_snapshot(),
            50,
            120,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );
        assert_eq!(lines.len(), 7);
        let halt_line = line_text(&lines[3]);
        assert!(halt_line.contains("Hlt:"));
        assert!(halt_line.contains("1/1"));
        assert!(halt_line.contains("0/1"));
    }

    #[test]
    fn match_strip_handles_missing_halt_history() {
        let mut snapshot = sample_snapshot();
        snapshot.a_halted.clear();
        snapshot.b_halted.clear();

        let lines = render_match_strip(
            &snapshot,
            50,
            120,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );
        let halt_line = line_text(&lines[3]);
        assert!(halt_line.contains("--"));
    }

    #[test]
    fn progress_waiting_text_reflects_running_status() {
        assert_eq!(
            progress_waiting_text(GamesStatus::Running),
            "Starting tournament..."
        );
        assert_eq!(
            progress_pending_round_text(GamesStatus::Running),
            "Round pending..."
        );
    }

    #[test]
    fn progress_waiting_text_reflects_done_status() {
        assert_eq!(
            progress_waiting_text(GamesStatus::Done),
            "Tournament complete"
        );
    }

    #[test]
    fn tm_family_prep_summary_reports_cpu_fallback_reason() {
        let timings = FamilyRunBuildTimings {
            tm_filter: Some(nit_games::TmHaltingFilterDiagnostics {
                backend: nit_games::TmHaltingFilterBackend::NotebookCpuFallback,
                metal_decline_reason: Some("non-zero noise disables Metal batch evaluation".into()),
                ..nit_games::TmHaltingFilterDiagnostics::default()
            }),
            ..FamilyRunBuildTimings::default()
        };

        let summary = tm_family_prep_summary(&timings).expect("expected prep summary");
        assert_eq!(
            summary,
            "prep fell back to CPU: non-zero noise disables Metal batch evaluation"
        );
    }

    #[test]
    fn family_build_result_loading_message_includes_tm_prep_summary() {
        let mut state = AppState::new(
            std::env::temp_dir(),
            nit_core::Buffer::empty("x", None),
            nit_core::Buffer::empty("n", None),
        );
        let mut runtime = GamesPetriDishRuntime::new(&state);
        state.games.family_building = true;

        let (tx, rx) = mpsc::channel();
        runtime.family_run_rx = Some(rx);
        let timings = FamilyRunBuildTimings {
            generation_elapsed: Duration::from_millis(1),
            estimate_elapsed: Duration::from_millis(2),
            normalize_elapsed: Duration::from_millis(3),
            tm_filter_elapsed: Some(Duration::from_millis(4)),
            tm_filter: Some(nit_games::TmHaltingFilterDiagnostics {
                backend: nit_games::TmHaltingFilterBackend::NotebookCpuFallback,
                metal_decline_reason: Some("non-zero noise disables Metal batch evaluation".into()),
                ..nit_games::TmHaltingFilterDiagnostics::default()
            }),
            ..FamilyRunBuildTimings::default()
        };
        let outcome = FamilyBuildOutcome {
            force: false,
            result: Ok(GamesRunOverride {
                config: sample_config(),
                config_text: "schema_version = 1".into(),
                label: "tm {1, 2}".into(),
                family_mode: true,
            }),
            timings: Some(timings),
        };
        tx.send(outcome).expect("send family build outcome");

        runtime.handle_family_build_result(&mut state);
        let message = runtime
            .loading_message
            .as_deref()
            .expect("expected loading message");
        assert!(message.contains("Queued tournament"));
        assert!(message.contains("tm-filter"));
        assert!(message.contains("prep fell back to CPU"));
        assert!(message.contains("non-zero noise"));
    }

    #[test]
    fn format_tournament_elapsed_uses_readable_units() {
        assert_eq!(
            format_tournament_elapsed(Duration::from_millis(875)),
            "875ms"
        );
        assert_eq!(
            format_tournament_elapsed(Duration::from_millis(12_345)),
            "12.345s"
        );
        assert_eq!(
            format_tournament_elapsed(Duration::from_millis(125_678)),
            "2m05.678s"
        );
    }

    #[test]
    fn finished_session_elapsed_stays_frozen() {
        let frozen = Duration::from_millis(12_345);
        let session = GameSession {
            config: sample_config(),
            progress: None,
            snapshot: None,
            results: TournamentResults::empty(),
            definitions: Vec::new(),
            started_at: Instant::now() - Duration::from_secs(90),
            finished_elapsed: Some(frozen),
        };

        assert_eq!(
            session.elapsed_at(Instant::now() + Duration::from_secs(30)),
            frozen
        );
    }

    #[test]
    fn session_footer_line_shows_elapsed_runtime() {
        let line = session_footer_line(
            2_048,
            false,
            false,
            Duration::from_millis(12_345),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert_eq!(
            line_text(&line),
            "steps/tick: 2048  paused: no  elapsed: 12.345s"
        );
    }

    #[test]
    fn session_footer_line_can_show_match_units() {
        let line = session_footer_line(
            32,
            true,
            false,
            Duration::from_millis(12_345),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert_eq!(
            line_text(&line),
            "matches/tick: 32  paused: no  elapsed: 12.345s"
        );
    }

    #[test]
    fn increase_steps_per_tick_clamps_normal_round_mode() {
        assert_eq!(increase_steps_per_tick(199, false), 200);
        assert_eq!(increase_steps_per_tick(200, false), 200);
    }

    #[test]
    fn increase_steps_per_tick_preserves_large_existing_values() {
        assert_eq!(increase_steps_per_tick(50_000, false), 50_001);
        assert_eq!(increase_steps_per_tick(4_096, true), 4_097);
    }

    #[test]
    fn increase_steps_per_tick_does_not_overflow() {
        assert_eq!(increase_steps_per_tick(u32::MAX, false), u32::MAX);
        assert_eq!(increase_steps_per_tick(u32::MAX, true), u32::MAX);
    }

    #[test]
    fn decrease_steps_per_tick_stops_at_one() {
        assert_eq!(decrease_steps_per_tick(2), 1);
        assert_eq!(decrease_steps_per_tick(1), 1);
    }

    #[test]
    fn tournament_progress_percent_uses_overall_progress() {
        let progress = TournamentProgress {
            match_index: 100_001,
            total_matches: 456_490,
            round: 0,
            rounds: 20,
            match_complete: false,
            a: "a".into(),
            b: "b".into(),
            total_payoff_a: 0,
            total_payoff_b: 0,
            last_action_a: None,
            last_action_b: None,
            last_payoff_a: None,
            last_payoff_b: None,
            last_halted_a: None,
            last_halted_b: None,
            last_outcome: None,
            runtime: nit_games::RuntimeAcceleratorStats::default(),
        };
        let pct = tournament_progress_percent(&progress);
        assert!(pct > 20.0);
        assert!(pct < 22.5);
    }

    #[test]
    fn render_progress_labels_completed_batch_snapshot_without_round_counter() {
        let mut state = AppState::new(
            std::env::temp_dir(),
            nit_core::Buffer::empty("x", None),
            nit_core::Buffer::empty("n", None),
        );
        state.games.status = GamesStatus::Running;
        let progress = TournamentProgress {
            match_index: 345,
            total_matches: 1000,
            round: 200,
            rounds: 200,
            match_complete: true,
            a: "a".into(),
            b: "b".into(),
            total_payoff_a: 0,
            total_payoff_b: -600,
            last_action_a: Some(nit_games::Action::Defect),
            last_action_b: Some(nit_games::Action::Cooperate),
            last_payoff_a: Some(0),
            last_payoff_b: Some(-3),
            last_halted_a: Some(true),
            last_halted_b: Some(true),
            last_outcome: Some(nit_games::game::Outcome::DC),
            runtime: {
                let mut runtime = nit_games::RuntimeAcceleratorStats::default();
                runtime.note_metal_policy(
                    131_072,
                    4,
                    nit_games::BatchPolicySource::Cached,
                    Some("apple_m4_max_demo".into()),
                    Some("/tmp/apple_m4_max_demo_v1.json".into()),
                );
                runtime
            },
        };

        let lines = render_progress(
            Some(progress),
            &[],
            &state,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );
        let match_line = line_text(&lines[1]);
        assert!(match_line.contains("last complete"));
        assert!(!match_line.contains("round 200/200"));
        let live_line = line_text(&lines[2]);
        assert!(live_line.contains("Showing last completed match snapshot"));
        assert!(!live_line.contains("GPU batching"));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("policy 131072x4 cached")));
        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("AccelCache: /tmp/apple_m4_max_demo_v1.json")));
    }

    #[test]
    fn family_mode_disables_history_to_preserve_metal_batching() {
        let mut state = AppState::new(
            std::env::temp_dir(),
            nit_core::Buffer::empty("x", None),
            nit_core::Buffer::empty("n", None),
        );
        let mut runtime = GamesPetriDishRuntime::new(&state);
        state.games.pending_run_override = Some(nit_core::GamesRunOverride {
            config: sample_config(),
            config_text: "schema_version = 1".into(),
            label: "fsm family".into(),
            family_mode: true,
        });

        runtime.start_session(&mut state);

        let session = runtime.session.as_ref().expect("session started");
        assert!(matches!(
            session.config.engine.mode,
            nit_games::EngineMode::Batch
        ));
        assert!(session.config.engine.fast_eval);
        assert!(!session.config.event_log.enabled);
        assert!(!session.config.history.enabled);
        assert!(state.games.match_history.capture_disabled_for_run);
    }

    #[test]
    fn finished_hidden_session_reopens_games_petri_dish() {
        let mut state = AppState::new(
            std::env::temp_dir(),
            nit_core::Buffer::empty("x", None),
            nit_core::Buffer::empty("n", None),
        );
        let mut runtime = GamesPetriDishRuntime::new(&state);
        runtime.session = Some(GameSession {
            config: sample_config(),
            progress: None,
            snapshot: None,
            results: TournamentResults::empty(),
            definitions: Vec::new(),
            started_at: Instant::now(),
            finished_elapsed: None,
        });
        runtime.hidden = true;
        state.games.running = true;
        state.games.status = GamesStatus::Running;
        state.games.petri_hidden = true;

        runtime.finish_session(
            &mut state,
            nit_games::output::RunSummary {
                schema_version: nit_games::output::RUN_SUMMARY_SCHEMA_VERSION,
                timestamp: "2026-03-11T12:00:00Z".into(),
                run_id: "run".into(),
                seed: 7,
                config_text: String::new(),
                config: sample_config(),
                paths: nit_games::output::RunPaths {
                    summary: None,
                    events: None,
                    history: None,
                    definitions: None,
                    results: None,
                    config: None,
                    analysis_dir: None,
                },
                strategies: Vec::new(),
                results: TournamentResults::empty(),
                event_log: None,
                history_log: None,
                runtime: nit_games::RuntimeAcceleratorStats::default(),
                run_dir: None,
            },
        );

        assert_eq!(state.games.status, GamesStatus::Done);
        assert!(!runtime.hidden);
        assert!(!state.games.petri_hidden);
        assert!(runtime.is_visible());
    }

    #[test]
    fn top_table_shows_aggregate_payoff_column_in_mean_mode() {
        let config = nit_games::GamesConfig::from_toml(
            r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#,
        )
        .expect("parse config");
        let kernel = nit_games::TournamentKernel::new(config.clone());
        let results = nit_games::output::TournamentResults {
            ranking: vec![nit_games::output::StrategyResult {
                id: "all_c".into(),
                name: None,
                total_payoff: -4,
                average_payoff: -1.0,
                adjusted_total_payoff: Some(-4.0),
                adjusted_average_payoff: Some(-1.0),
                matches: 2,
                wins: 0,
                losses: 0,
                draws: 2,
                crashed: false,
                crash_count: 0,
                tm_metrics: None,
            }],
            pairwise: Vec::new(),
            dominance: Vec::new(),
        };
        let (rank_w, score_w, total_w, wld_w) = top_table_widths(&config);
        let lines = render_top_table(
            &results,
            &config,
            kernel.definitions(),
            120,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            rank_w,
            score_w,
            total_w,
            wld_w,
        );

        assert!(lines
            .iter()
            .any(|line| line_text(line).contains("AggPayoff")));
        assert!(lines.iter().any(|line| line_text(line).contains(" -2 ")));
    }

    #[test]
    fn wolfram_preview_line_is_expression_friendly() {
        let preview = nit_games::MatchHistoryPreview {
            match_index: 53,
            total_matches: 913_936,
            a: "0".into(),
            b: "867".into(),
            rounds_total: 4,
            outcomes: "2323".into(),
        };

        let line = wolfram_preview_line(&preview);
        assert_eq!(
            line,
            "<|\"match_index\" -> 53, \"total_matches\" -> 913936, \"a\" -> \"0\", \"b\" -> \"867\", \"rounds_total\" -> 4, \"outcomes\" -> \"2323\"|>"
        );
    }
}
