use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{AppState, GamesAnalysisRequest, GamesStatus, UiSelectionPane};
use nit_games::output::StrategyDefinition;
use nit_games::{
    config::GamesConfig,
    events::EventWriter,
    output::{RunLayout, TournamentResults},
    run_id_from_seed_config, MatchSnapshot, TournamentProgress,
};
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
use crate::games_runner::{GamesRunner, RunRequest, RunnerCommand, RunnerEvent};
use crate::games_runs::{GamesRunsRunner, RunsCommand, RunsEvent};
use crate::theme::Theme;
use crate::widgets::games_visualizer_view::strategy_display_name_from_def;
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 88;
const MIN_HEIGHT: u16 = 22;

pub fn petri_rect(screen: Rect) -> Rect {
    let width = screen.width.min(MIN_WIDTH).max(60);
    let height = screen.height.min(MIN_HEIGHT).max(16);
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
}

pub struct GamesPetriDishRuntime {
    runner: GamesRunner,
    analysis_runner: GamesAnalysisRunner,
    runs_runner: GamesRunsRunner,
    session: Option<GameSession>,
    hidden: bool,
    warning: Option<String>,
    last_tick: Instant,
    view: PetriView,
    inspector_window: usize,
}

struct GameSession {
    config: nit_games::NormalizedConfig,
    progress: Option<TournamentProgress>,
    snapshot: Option<MatchSnapshot>,
    results: TournamentResults,
    definitions: Vec<StrategyDefinition>,
}

impl GamesPetriDishRuntime {
    pub fn new(_state: &AppState) -> Self {
        Self {
            runner: GamesRunner::spawn(),
            analysis_runner: GamesAnalysisRunner::spawn(),
            runs_runner: GamesRunsRunner::spawn(),
            session: None,
            hidden: false,
            warning: None,
            last_tick: Instant::now(),
            view: PetriView::Tournament,
            inspector_window: 50,
        }
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    pub fn is_visible(&self) -> bool {
        self.session.is_some() && !self.hidden
    }

    pub fn handle_pending_requests(&mut self, state: &mut AppState) {
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

    pub fn handle_key(&mut self, key: &KeyEvent, state: &mut AppState) -> bool {
        if self.warning.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ')) {
                self.warning = None;
                return true;
            }
            return true;
        }
        let Some(_session) = self.session.as_mut() else {
            return false;
        };

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
                    PetriView::Inspector => PetriView::Tournament,
                };
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
            KeyCode::Esc => {
                self.close(state);
                true
            }
            KeyCode::Char(' ') => {
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
                state.games.steps_per_tick = (state.games.steps_per_tick + 1).min(200);
                self.runner
                    .send(RunnerCommand::UpdateSpeed(state.games.steps_per_tick));
                true
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                state.games.steps_per_tick = state.games.steps_per_tick.saturating_sub(1).max(1);
                self.runner
                    .send(RunnerCommand::UpdateSpeed(state.games.steps_per_tick));
                true
            }
            _ => false,
        }
    }

    pub fn tick(&mut self, state: &mut AppState) {
        self.handle_analysis_events(state);
        self.handle_runs_events(state);
        self.handle_runner_events(state);
        self.last_tick = Instant::now();
    }

    fn handle_analysis_events(&mut self, state: &mut AppState) {
        while let Ok(event) = self.analysis_runner.events.try_recv() {
            match event {
                AnalysisEvent::Started(request) => {
                    state.games.analysis.running = true;
                    state.games.analysis.last_error = None;
                    state.games.analysis.source_path =
                        Some(request.history_path.display().to_string());
                    state.status = Some("Games analysis started".into());
                }
                AnalysisEvent::Finished(result) => {
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
                    let pairs = summary
                        .results
                        .pairwise
                        .iter()
                        .map(|p| (p.a.clone(), p.b.clone()))
                        .collect::<Vec<_>>();
                    state.games.last_run_path = summary.paths.summary.clone();
                    state.games.last_event_path = summary.event_log.clone();
                    state.games.last_history_path = summary.history_log.clone();
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
            match event {
                RunnerEvent::Definitions(defs) => {
                    if let Some(session) = self.session.as_mut() {
                        session.definitions = defs;
                    }
                }
                RunnerEvent::Progress(progress) => {
                    if let Some(session) = self.session.as_mut() {
                        session.progress = Some(progress);
                    }
                }
                RunnerEvent::MatchPreview(snapshot) => {
                    if let Some(session) = self.session.as_mut() {
                        session.snapshot = Some(snapshot);
                    }
                }
                RunnerEvent::PartialLeaderboard(results) => {
                    if let Some(session) = self.session.as_mut() {
                        session.results = results;
                    }
                }
                RunnerEvent::Finished(summary) => {
                    let pairs = summary
                        .results
                        .pairwise
                        .iter()
                        .map(|p| (p.a.clone(), p.b.clone()))
                        .collect::<Vec<_>>();
                    self.session = None;
                    state.games.last_error = None;
                    state.games.last_run_path = summary.paths.summary.clone();
                    state.games.last_event_path = summary.event_log.clone();
                    state.games.last_history_path = summary.history_log.clone();
                    state.games.last_run = Some(summary);
                    state.games.replay.pairs = pairs;
                    reset_strategy_inspect(state);
                    state.games.status = GamesStatus::Done;
                    state.games.running = false;
                    state.games.paused = false;
                    state.games.petri_hidden = false;
                    state.games.petri_lines.clear();
                    state.status = Some("Games tournament completed".into());
                }
                RunnerEvent::Cancelled => {
                    self.session = None;
                    state.games.last_error = None;
                    state.games.running = false;
                    state.games.paused = false;
                    state.games.status = GamesStatus::Idle;
                    state.games.petri_hidden = false;
                    state.games.petri_lines.clear();
                    state.status = Some("Games tournament cancelled".into());
                }
                RunnerEvent::Error(err) => {
                    self.session = None;
                    state.games.running = false;
                    state.games.paused = false;
                    state.games.status = GamesStatus::Error;
                    state.games.last_error = Some(err.clone());
                    state.games.petri_lines.clear();
                    state.status = Some(err);
                }
            }
        }
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
        if let Some(warning) = self.warning.as_ref() {
            lines.push(Line::from(Span::styled(
                warning.clone(),
                Style::default().fg(theme.warning),
            )));
        } else if let Some(session) = self.session.as_ref() {
            match self.view {
                PetriView::Tournament => {
                    let progress = session.progress.clone();
                    lines.extend(render_progress(
                        progress,
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
                    let (rank_w, score_w, wld_w) = top_table_widths(&session.config);
                    lines.extend(render_top_table(
                        results,
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
                        wld_w,
                    ));
                }
                PetriView::Inspector => {
                    let snapshot = session.snapshot.clone();
                    lines.extend(render_match_inspector(
                        snapshot,
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
            }
            lines.push(Line::from(""));
            let paused_style = if state.games.paused {
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.accent)
            };
            lines.push(Line::from(vec![
                Span::styled("steps/tick: ", label_style),
                Span::styled(state.games.steps_per_tick.to_string(), number_style),
                Span::styled("  ", dim_style),
                Span::styled("paused: ", label_style),
                Span::styled(if state.games.paused { "yes" } else { "no" }, paused_style),
            ]));
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
                    Span::styled("←/→", key_style),
                    Span::styled(" window | ", help_style),
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
        if self.session.is_some() {
            self.warning = Some("Games tournament already running".into());
            return;
        }
        let config_text = state.editor_buffer().content_as_string();
        let mut config =
            match GamesConfig::from_toml_with_root(&config_text, Some(&state.workspace_root)) {
                Ok(config) => config,
                Err(err) => {
                    let msg = format!("Config error: {err}");
                    state.games.status = GamesStatus::Error;
                    state.games.last_error = Some(msg.clone());
                    state.status = Some(msg);
                    return;
                }
            };

        config.engine.mode = nit_games::EngineMode::Interactive;

        let timestamp = EventWriter::timestamp();
        let seed = config
            .seed
            .unwrap_or_else(|| stable_hash_bytes(format!("{timestamp}\n{config_text}").as_bytes()));
        config.seed = Some(seed);

        let run_id = run_id_from_seed_config(seed, &config_text);
        let layout = RunLayout::for_base(&state.workspace_root, &timestamp, seed, &run_id);
        if let Err(err) = fs::create_dir_all(&layout.run_dir) {
            let msg = format!("Failed to create games runs: {err}");
            state.games.status = GamesStatus::Error;
            state.games.last_error = Some(msg.clone());
            state.status = Some(msg);
            return;
        }

        let event_log_enabled = config.event_log.enabled;
        let history_log_enabled = config.history.enabled;
        let summary_path = layout.summary_path.clone();
        let event_path = layout.events_path.clone();
        let history_path = layout.history_path.clone();
        info!("Games summary path: {}", summary_path.display());
        if event_log_enabled {
            info!("Games event log path: {}", event_path.display());
        }
        if history_log_enabled {
            info!("Games history log path: {}", history_path.display());
        }
        let progress_interval =
            std::time::Duration::from_millis(config.engine.progress_interval_ms);
        let request = RunRequest {
            config: config.clone(),
            config_text: config_text.clone(),
            timestamp: timestamp.clone(),
            run_id: run_id.clone(),
            run_dir: layout.run_dir.clone(),
            summary_path: summary_path.clone(),
            definitions_path: layout.definitions_path.clone(),
            results_path: layout.results_path.clone(),
            config_path: layout.config_path.clone(),
            analysis_dir: layout.analysis_dir.clone(),
            event_path: event_log_enabled.then_some(event_path),
            history_path: history_log_enabled.then_some(history_path),
            progress_interval,
            steps_per_tick: state.games.steps_per_tick.max(1),
        };

        self.runner.send(RunnerCommand::StartRun(request));
        self.session = Some(GameSession {
            config,
            progress: None,
            snapshot: None,
            results: TournamentResults::empty(),
            definitions: Vec::new(),
        });
        self.hidden = false;
        state.games.petri_hidden = false;
        state.games.running = true;
        state.games.paused = false;
        state.games.status = GamesStatus::Running;
        state.games.last_error = None;
        state.status = Some("Games tournament started".into());
        info!("Games tournament started");
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
        self.session = None;
        self.hidden = false;
        state.games.running = false;
        state.games.paused = false;
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
        if self.session.is_some() {
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
        if self.session.is_some() {
            self.hidden = false;
            state.games.petri_hidden = false;
        }
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
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        }) {
            tracing::warn!("Failed to write games definitions: {err}");
        }
        if let Err(err) = nit_utils::fs::write_atomic(&layout.results_path, |writer| {
            serde_json::to_writer_pretty(writer, &export_run.results)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
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

fn render_progress(
    progress: Option<TournamentProgress>,
    state: &AppState,
    label_style: Style,
    value_style: Style,
    number_style: Style,
    dim_style: Style,
    status_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Status: ", label_style),
        Span::styled(format!("{:?}", state.games.status), status_style),
    ]));
    if let Some(progress) = progress {
        let pct = if progress.rounds > 0 {
            (progress.round as f32 / progress.rounds as f32) * 100.0
        } else {
            0.0
        };
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled(progress.match_index.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(progress.total_matches.to_string(), number_style),
            Span::styled(" (round ", dim_style),
            Span::styled(progress.round.to_string(), number_style),
            Span::styled("/", dim_style),
            Span::styled(progress.rounds.to_string(), number_style),
            Span::styled(", ", dim_style),
            Span::styled(format!("{:>5.1}%", pct), number_style),
            Span::styled(")", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled(progress.a, value_style),
            Span::styled(" vs ", dim_style),
            Span::styled(progress.b, value_style),
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
                    Span::styled("Waiting for tournament...", dim_style),
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
                Span::styled("Waiting for tournament...", dim_style),
            ]));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled("Waiting for tournament...", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled("Waiting for tournament...", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Last: ", label_style),
            Span::styled("Waiting for tournament...", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Payoff: ", label_style),
            Span::styled("Waiting for tournament...", dim_style),
        ]));
    }
    lines
}

fn render_match_inspector(
    snapshot: Option<MatchSnapshot>,
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
    lines.push(Line::from(Span::styled("Match Inspector", header_style)));
    lines.push(Line::from(vec![
        Span::styled("window: ", label_style),
        Span::styled(window.to_string(), number_style),
        Span::styled(" rounds", dim_style),
    ]));

    if let Some(snapshot) = snapshot {
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
            Span::styled(snapshot.a.clone(), value_style),
            Span::styled(" vs ", dim_style),
            Span::styled(snapshot.b.clone(), value_style),
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
    } else {
        lines.push(Line::from(vec![
            Span::styled("Match: ", label_style),
            Span::styled("Waiting for tournament...", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Pair: ", label_style),
            Span::styled("Waiting for tournament...", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Score: ", label_style),
            Span::styled("Waiting for tournament...", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Outcomes: ", label_style),
            Span::styled("Waiting for tournament...", dim_style),
        ]));
    }
    lines
}

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

    let label_w = 3usize;
    let prefix_len = label_w + 2;
    let available = width.saturating_sub(prefix_len);
    let mut max_len = 3usize;
    let window_start = total.saturating_sub(window);
    for i in window_start..total {
        let round_len = (i + 1).to_string().chars().count();
        let idx_char = snapshot.outcomes.as_bytes()[i] as char;
        let out_len = match idx_char {
            '0' | '1' | '2' | '3' => 3,
            _ => 2,
        };
        let payoff = snapshot.payoffs[i];
        let payoff_len = format!("{}/{}", payoff[0], payoff[1]).chars().count();
        let total_len = format!("{}/{}", cumulative[i].0, cumulative[i].1)
            .chars()
            .count();
        max_len = max_len
            .max(round_len)
            .max(out_len)
            .max(payoff_len)
            .max(total_len);
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
    let mut pay_line = String::new();
    let mut total_line = String::new();
    out_spans.push(Span::styled(
        format!("{:>label_w$}: ", "Out", label_w = label_w),
        label_style,
    ));
    for i in start..total {
        idx_line.push_str(&fit_right(&(i + 1).to_string()));
        let idx_char = snapshot.outcomes.as_bytes()[i] as char;
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
        let payoff = snapshot.payoffs[i];
        pay_line.push_str(&fit_right(&format!("{}/{}", payoff[0], payoff[1])));
        total_line.push_str(&fit_right(&format!(
            "{}/{}",
            cumulative[i].0, cumulative[i].1
        )));
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

fn top_table_widths(config: &nit_games::NormalizedConfig) -> (usize, usize, usize) {
    let n = config.strategies.len().max(1);
    let matches_per = if config.self_play {
        n
    } else {
        n.saturating_sub(1)
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
    let max_total = max_abs
        .saturating_mul(matches_per as i64)
        .saturating_mul(rounds);
    let score_w = if min_payoff < 0 {
        (-max_total).to_string().len()
    } else {
        max_total.to_string().len()
    };
    let rank_w = n.to_string().len();
    let wld_w = format!("W{}-L{}-D{}", matches_per, matches_per, matches_per).len();
    (rank_w, score_w, wld_w)
}

fn render_top_table(
    results: &nit_games::output::TournamentResults,
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
    fixed_wld_w: usize,
) -> Vec<Line<'static>> {
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

    let rows: Vec<(String, String, String, String, u32, u32, u32)> = results
        .ranking
        .iter()
        .take(5)
        .enumerate()
        .map(|(idx, entry)| {
            let display = definitions
                .iter()
                .find(|def| def.id == entry.id)
                .map(strategy_display_name_from_def)
                .unwrap_or_else(|| entry.id.clone());
            let rank = format!("{}", idx + 1);
            let id = entry.id.clone();
            let score = format!("{}", entry.total_payoff);
            (
                rank,
                id,
                display,
                score,
                entry.wins,
                entry.losses,
                entry.draws,
            )
        })
        .collect();

    let headers = ["#", "id", "Strategy", "Score", "W-L-D"];
    let mut rank_w = headers[0].len().max(fixed_rank_w);
    let mut id_w = headers[1].len();
    let mut name_w = headers[2].len();
    let mut score_w = headers[3].len().max(fixed_score_w);
    let mut wld_w = headers[4].len().max(fixed_wld_w);

    for (rank, id, name, score, wins, losses, draws) in &rows {
        rank_w = rank_w.max(rank.len());
        id_w = id_w.max(id.len());
        name_w = name_w.max(name.chars().count());
        score_w = score_w.max(score.len());
        let wld_len = format!("W{}-L{}-D{}", wins, losses, draws).len();
        wld_w = wld_w.max(wld_len);
    }

    let min_id = 4usize;
    let min_name = 10usize;
    let overhead = 6 + (2 * 5);
    let fixed = rank_w + score_w + wld_w;
    let available = width.saturating_sub(overhead + fixed);
    if available >= min_id + min_name {
        id_w = id_w.min(available.saturating_sub(min_name));
        name_w = name_w.min(available.saturating_sub(id_w));
    } else {
        id_w = id_w.min(available.saturating_sub(1).max(1));
        name_w = available.saturating_sub(id_w).max(1);
    }

    let sep = format!(
        "+{}+{}+{}+{}+{}+",
        "-".repeat(rank_w + 2),
        "-".repeat(id_w + 2),
        "-".repeat(name_w + 2),
        "-".repeat(score_w + 2),
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
        Span::styled(
            format!(" {} ", center_text(headers[2], name_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[3], score_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(headers[4], wld_w)),
            header_style,
        ),
        Span::styled("|", dim_style),
    ]));
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));

    for (rank, id, name, score, wins, losses, draws) in rows {
        let mut spans = Vec::new();
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:>width$} ", rank, width = rank_w),
            number_style,
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:<width$} ", truncate_text(&id, id_w), width = id_w),
            label_style,
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:<width$} ", truncate_text(&name, name_w), width = name_w),
            value_style,
        ));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(
            format!(" {:>width$} ", score, width = score_w),
            number_style,
        ));
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

    lines.push(Line::from(Span::styled(sep, dim_style)));
    lines
}

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
    let base = format!("W{}-L{}-D{}", wins, losses, draws);
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
