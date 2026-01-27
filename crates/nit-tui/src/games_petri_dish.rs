use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{AppState, GamesStatus};
use nit_games::{
    config::GamesConfig,
    events::EventWriter,
    output::RunSummary,
    tournament::{TournamentProgress, TournamentRunner},
    HistoryWriter,
};
use nit_utils::fs::write_atomic;
use nit_utils::hashing::stable_hash_bytes;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use tracing::{info, warn};

use crate::theme::Theme;
use crate::widgets::games_visualizer_view::strategy_display_name_from_def;

const MIN_WIDTH: u16 = 88;
const MIN_HEIGHT: u16 = 22;

pub struct GamesPetriDishRuntime {
    session: Option<GameSession>,
    hidden: bool,
    warning: Option<String>,
    last_tick: Instant,
}

struct GameSession {
    runner: TournamentRunner,
    timestamp: String,
    summary_path: PathBuf,
}

impl GamesPetriDishRuntime {
    pub fn new(_state: &AppState) -> Self {
        Self {
            session: None,
            hidden: false,
            warning: None,
            last_tick: Instant::now(),
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
                self.step_once(state);
            }
            return true;
        }

        match key.code {
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
                true
            }
            KeyCode::Enter => {
                if state.games.paused {
                    self.step_once(state);
                }
                true
            }
            KeyCode::Char('h') | KeyCode::Char('H') => {
                self.hide(state);
                true
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                state.games.steps_per_tick = (state.games.steps_per_tick + 1).min(200);
                true
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                state.games.steps_per_tick = state.games.steps_per_tick.saturating_sub(1).max(1);
                true
            }
            _ => false,
        }
    }

    pub fn tick(&mut self, state: &mut AppState) {
        if self.session.is_none() {
            return;
        }
        if state.games.paused {
            return;
        }
        let steps = state.games.steps_per_tick.max(1);
        if let Some(session) = self.session.as_mut() {
            session.runner.step_rounds(steps);
            if session.runner.is_done() {
                self.finish_session(state);
            }
        }
        self.last_tick = Instant::now();
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect, state: &AppState, theme: &Theme) {
        if !self.is_visible() {
            return;
        }
        let width = screen.width.min(MIN_WIDTH).max(60);
        let height = screen.height.min(MIN_HEIGHT).max(16);
        let area = Rect {
            x: screen.x + (screen.width.saturating_sub(width)) / 2,
            y: screen.y + (screen.height.saturating_sub(height)) / 2,
            width,
            height,
        };
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
            let progress = session.runner.progress();
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
            let results = session.runner.results();
            let definitions = session.runner.definitions();
            let (rank_w, score_w, wld_w) = top_table_widths(&session.runner);
            lines.extend(render_top_table(
                &results,
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
            lines.push(Line::from(vec![
                Span::styled("Esc", key_style),
                Span::styled(" close | ", help_style),
                Span::styled("Space", key_style),
                Span::styled(" pause | ", help_style),
                Span::styled("Enter", key_style),
                Span::styled(" step | ", help_style),
                Span::styled("+/-", key_style),
                Span::styled(" speed | ", help_style),
                Span::styled("H", key_style),
                Span::styled(" hide", help_style),
            ]));
        }

        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .block(Block::default().style(Style::default().bg(theme.background)));
        frame.render_widget(paragraph, inner);
    }

    fn start_session(&mut self, state: &mut AppState) {
        let config_text = state.editor_buffer().content_as_string();
        let mut config = match GamesConfig::from_toml(&config_text) {
            Ok(config) => config,
            Err(err) => {
                let msg = format!("Config error: {err}");
                state.games.status = GamesStatus::Error;
                state.games.last_error = Some(msg.clone());
                state.status = Some(msg);
                return;
            }
        };

        let timestamp = EventWriter::timestamp();
        let seed = config
            .seed
            .unwrap_or_else(|| stable_hash_bytes(format!("{timestamp}\n{config_text}").as_bytes()));
        config.seed = Some(seed);

        let run_hash = stable_hash_bytes(format!("{seed}:{config_text}").as_bytes());
        let stamp = timestamp.replace(':', "-");
        let run_dir = state.workspace_root.join("games-runs");
        if let Err(err) = fs::create_dir_all(&run_dir) {
            let msg = format!("Failed to create games-runs: {err}");
            state.games.status = GamesStatus::Error;
            state.games.last_error = Some(msg.clone());
            state.status = Some(msg);
            return;
        }

        let event_log_enabled = config.event_log.enabled;
        let include_rounds = config.event_log.include_rounds;
        let history_log_enabled = config.history.enabled;
        let summary_path = run_dir.join(format!("run__{stamp}__{run_hash:08x}.json"));
        let event_path = run_dir.join(format!("events__{stamp}__{run_hash:08x}.ndjson"));
        let history_path = run_dir.join(format!("history__{stamp}__{run_hash:08x}.ndjson"));
        info!("Games summary path: {}", summary_path.display());
        if event_log_enabled {
            info!("Games event log path: {}", event_path.display());
        }
        if history_log_enabled {
            info!("Games history log path: {}", history_path.display());
        }
        let mut runner = TournamentRunner::new(config);
        if event_log_enabled {
            match EventWriter::new(event_path.clone(), include_rounds) {
                Ok(writer) => runner = runner.with_event_writer(writer),
                Err(err) => warn!("Failed to create event log: {err}"),
            }
        }
        if history_log_enabled {
            match HistoryWriter::new(history_path.clone()) {
                Ok(writer) => runner = runner.with_history_writer(writer),
                Err(err) => warn!("Failed to create history log: {err}"),
            }
        }

        self.session = Some(GameSession {
            runner,
            timestamp,
            summary_path,
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

    fn finish_session(&mut self, state: &mut AppState) {
        let Some(session) = self.session.take() else {
            return;
        };
        let summary = session.runner.finish(session.timestamp.clone());
        if let Err(err) = write_summary(&session.summary_path, &summary) {
            warn!("Failed to write games summary: {err}");
            state.games.last_error = Some(err.to_string());
        } else {
            state.games.last_run_path = Some(session.summary_path.display().to_string());
            state.games.last_event_path = summary.event_log.clone();
            state.games.last_history_path = summary.history_log.clone();
        }
        state.games.last_run = Some(summary);
        state.games.status = GamesStatus::Done;
        state.games.running = false;
        state.games.paused = false;
        state.games.petri_hidden = false;
        state.status = Some("Games tournament completed".into());
        info!("Games tournament completed");
    }

    fn close(&mut self, state: &mut AppState) {
        self.session = None;
        self.hidden = false;
        state.games.running = false;
        state.games.paused = false;
        state.games.status = GamesStatus::Idle;
        state.games.petri_hidden = false;
    }

    fn hide(&mut self, state: &mut AppState) {
        if self.session.is_some() {
            self.hidden = true;
            state.games.petri_hidden = true;
        }
    }

    fn show(&mut self, state: &mut AppState) {
        if self.session.is_some() {
            self.hidden = false;
            state.games.petri_hidden = false;
        }
    }

    fn step_once(&mut self, state: &mut AppState) {
        if let Some(session) = self.session.as_mut() {
            session.runner.step_rounds(1);
            if session.runner.is_done() {
                self.finish_session(state);
            }
        }
    }

    fn export_last_run(&mut self, state: &mut AppState) {
        let Some(run) = state.games.last_run.as_ref() else {
            state.status = Some("No completed run to export".into());
            return;
        };
        let timestamp = EventWriter::timestamp();
        let run_hash = stable_hash_bytes(format!("{}:{timestamp}", run.seed).as_bytes());
        let stamp = timestamp.replace(':', "-");
        let run_dir = state.workspace_root.join("games-runs");
        if let Err(err) = fs::create_dir_all(&run_dir) {
            state.status = Some(format!("Failed to create games-runs: {err}"));
            return;
        }
        let summary_path = run_dir.join(format!("run__{stamp}__{run_hash:08x}.json"));
        if let Err(err) = write_summary(&summary_path, run) {
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
        if let (Some(a), Some(b)) = (progress.last_action_a, progress.last_action_b) {
            lines.push(Line::from(vec![
                Span::styled("Last: ", label_style),
                Span::styled(a.as_char().to_string(), number_style),
                Span::styled(" / ", dim_style),
                Span::styled(b.as_char().to_string(), number_style),
            ]));
        }
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

fn write_summary(path: &PathBuf, summary: &RunSummary) -> std::io::Result<()> {
    write_atomic(path, |writer| {
        serde_json::to_writer_pretty(writer, summary)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    })
}

fn top_table_widths(runner: &TournamentRunner) -> (usize, usize, usize) {
    let config = runner.config();
    let n = config.strategies.len().max(1);
    let matches_per = if config.self_play {
        n
    } else {
        n.saturating_sub(1)
    };
    let matches_per = matches_per.saturating_mul(config.repetitions.max(1) as usize);
    let rounds = config.rounds.max(1) as i64;
    let payoffs = [
        config.payoff.r,
        config.payoff.s,
        config.payoff.t,
        config.payoff.p,
    ];
    let max_payoff = payoffs.iter().copied().max().unwrap_or(0) as i64;
    let min_payoff = payoffs.iter().copied().min().unwrap_or(0) as i64;
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
