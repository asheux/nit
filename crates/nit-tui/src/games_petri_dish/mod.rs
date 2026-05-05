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
use crate::widgets::text_selection::apply_ui_selection;

mod render;
use render::{
    format_tournament_elapsed, lines_to_strings, normalize_path, render_cache_browser,
    render_match_inspector, render_progress, render_top_table, session_footer_line, status_style,
    top_table_widths,
};
#[cfg(test)]
pub(super) use render::{
    progress_pending_round_text, progress_waiting_text, render_match_strip,
    tournament_progress_percent,
};

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

/// UI-side coordinator that owns the background game runners and the games petri-dish popup state.
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
                            message.push_str(&format!(", tm-filter {tm_filter_elapsed:?}"));
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

#[cfg(test)]
#[path = "../tests/games_petri_dish.rs"]
mod tests;
