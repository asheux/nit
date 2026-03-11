use nit_core::{AppState, GamesStatus, PaneId, UiSelectionPane};
use nit_games::config::StrategySpecKind;
use nit_games::strategy::InputMode;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

const LAST_RUN_PANEL_TARGET_WIDTH: usize = 34;
const LAST_RUN_PANEL_EXTRA_WIDTH: usize = 6;

pub struct VisualizerLayout {
    pub main: Rect,
    pub side: Option<Rect>,
    pub show_payoff_side: bool,
}

pub fn layout_for_config(
    inner: Rect,
    _state: &AppState,
    config: Option<&nit_games::config::NormalizedConfig>,
) -> VisualizerLayout {
    let mut show_payoff_side = false;
    let (main_area, right_area) = if let Some(config) = config {
        let desired = payoff_panel_width(&config.payoff)
            .max(LAST_RUN_PANEL_TARGET_WIDTH)
            .saturating_add(2 + LAST_RUN_PANEL_EXTRA_WIDTH);
        let min_main = 32usize;
        if inner.width as usize >= min_main + desired {
            show_payoff_side = true;
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(min_main as u16),
                    Constraint::Length(desired as u16),
                ])
                .split(inner);
            (cols[0], cols[1])
        } else {
            (inner, Rect::default())
        }
    } else {
        (inner, Rect::default())
    };
    let side = if show_payoff_side {
        Some(right_area)
    } else {
        None
    };
    VisualizerLayout {
        main: main_area,
        side,
        show_payoff_side,
    }
}

pub fn build_main_lines(
    state: &AppState,
    theme: &Theme,
    config_result: Option<
        &Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>,
    >,
    config_pending: bool,
    show_payoff_side: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let title_color = if state.focus == PaneId::Visualizer {
        theme.title_focused
    } else {
        theme.title
    };
    let header_style = Style::default()
        .fg(title_color)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let file_dim_style = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::DIM);
    let number_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Games Dashboard", header_style),
        Span::raw("  "),
        Span::styled("Status: ", label_style),
        Span::styled(
            status_label(state.games.status),
            Style::default().fg(theme.accent),
        ),
    ]));
    if let Some(err) = state.games.last_error.as_ref() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Error: ", Style::default().fg(theme.warning)),
            Span::styled(err.clone(), value_style),
        ]));
    }

    if !show_payoff_side {
        lines.push(Line::from(""));
        lines.extend(last_run_lines(
            state,
            header_style,
            label_style,
            value_style,
            dim_style,
            file_dim_style,
            number_style,
            width,
        ));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Config Summary", header_style)));

    match config_result {
        Some(Ok(config)) => {
            lines.push(Line::from(vec![
                Span::styled("game: ", label_style),
                Span::styled(config.game.clone(), value_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("rounds: ", label_style),
                Span::styled(config.rounds.to_string(), value_style),
                Span::raw("  "),
                Span::styled("repetitions: ", label_style),
                Span::styled(config.repetitions.to_string(), value_style),
                Span::raw("  "),
                Span::styled("self_play: ", label_style),
                Span::styled(config.self_play.to_string(), value_style),
            ]));
            let seed_label = config
                .seed
                .map(|s| s.to_string())
                .unwrap_or_else(|| "auto".into());
            lines.push(Line::from(vec![
                Span::styled("seed: ", label_style),
                Span::styled(seed_label, value_style),
                Span::raw("  "),
                Span::styled("noise: ", label_style),
                Span::styled(format!("{:.3}", config.noise), value_style),
            ]));
            lines.extend(payoff_lines(
                &config.payoff,
                width,
                value_style,
                dim_style,
                label_style,
            ));
            lines.push(Line::from(vec![
                Span::styled("strategies: ", label_style),
                Span::styled(config.strategies.len().to_string(), value_style),
            ]));
            let interesting: Vec<&nit_games::config::StrategySpec> =
                config.strategies.iter().collect();
            if interesting.is_empty() {
                lines.push(Line::from(Span::styled(
                    "No complex strategies in config.",
                    dim_style,
                )));
            } else {
                lines.extend(render_strategy_table(
                    &interesting,
                    width,
                    label_style,
                    value_style,
                    dim_style,
                ));
            }
            lines.push(Line::from(Span::styled(
                "Use :games strategies all to list every strategy.",
                dim_style,
            )));
        }
        Some(Err(err)) => {
            lines.push(Line::from(vec![Span::styled(
                "Config error:",
                Style::default().fg(theme.warning),
            )]));
            for msg in err.errors.iter().take(6) {
                lines.push(Line::from(vec![
                    Span::styled("- ", dim_style),
                    Span::styled(msg.clone(), value_style),
                ]));
            }
        }
        None => {
            let label = if config_pending {
                "Parsing config in background..."
            } else {
                "Config preview pending..."
            };
            lines.push(Line::from(vec![
                Span::styled("Config: ", label_style),
                Span::styled(label, dim_style),
            ]));
        }
    }

    lines
}

pub fn build_side_lines(state: &AppState, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let title_color = if state.focus == PaneId::Visualizer {
        theme.title_focused
    } else {
        theme.title
    };
    let header_style = Style::default()
        .fg(title_color)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let file_dim_style = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::DIM);
    let number_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    last_run_lines(
        state,
        header_style,
        label_style,
        value_style,
        dim_style,
        file_dim_style,
        number_style,
        width,
    )
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    config_result: Option<
        &Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>,
    >,
    config_pending: bool,
) {
    let focused = state.focus == PaneId::Visualizer;
    let border = if focused {
        Style::default().fg(theme.title_focused)
    } else {
        Style::default().fg(theme.border)
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            " VISUALIZER ",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = layout_for_config(
        inner,
        state,
        config_result.and_then(|result| result.as_ref().ok()),
    );

    let mut lines = build_main_lines(
        state,
        theme,
        config_result,
        config_pending,
        layout.show_payoff_side,
        layout.main.width as usize,
    );
    lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::VisualizerMain,
        theme.selection_bg,
        0,
    );
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, layout.main);

    if let Some(right_area) = layout.side {
        let right_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .style(Style::default().bg(theme.background));
        let right_inner = right_block.inner(right_area);
        frame.render_widget(right_block, right_area);
        if right_inner.width > 0 && right_inner.height > 0 {
            let mut side_lines = build_side_lines(state, theme, right_inner.width as usize);
            side_lines = apply_ui_selection(
                side_lines,
                state.ui_selection.as_ref(),
                UiSelectionPane::VisualizerSide,
                theme.selection_bg,
                0,
            );
            let right_paragraph = Paragraph::new(side_lines)
                .style(Style::default().fg(theme.foreground).bg(theme.background))
                .wrap(Wrap { trim: true });
            frame.render_widget(right_paragraph, right_inner);
        }
    }
}

fn status_label(status: GamesStatus) -> &'static str {
    match status {
        GamesStatus::Idle => "IDLE",
        GamesStatus::Running => "RUNNING",
        GamesStatus::Paused => "PAUSED",
        GamesStatus::Done => "DONE",
        GamesStatus::Error => "ERROR",
    }
}

pub fn strategy_display_name(strategy: &nit_games::config::StrategySpec) -> String {
    if let Some(name) = strategy.name.as_ref() {
        return name.clone();
    }
    strategy_display_name_from_kind(&strategy.kind)
}

pub fn strategy_display_name_from_def(def: &nit_games::output::StrategyDefinition) -> String {
    if let Some(name) = def.name.as_ref() {
        return name.clone();
    }
    strategy_display_name_from_kind(&def.kind)
}

fn strategy_kind_label(kind: &StrategySpecKind) -> &'static str {
    match kind {
        StrategySpecKind::Fsm { .. } => "fsm",
        StrategySpecKind::Ca { .. } => "ca",
        StrategySpecKind::OneSidedTm { .. } => "tm",
    }
}

fn strategy_display_name_from_kind(kind: &StrategySpecKind) -> String {
    match kind {
        StrategySpecKind::Fsm {
            outputs,
            num_states,
            input_mode,
            ..
        } => {
            let states = if !outputs.is_empty() {
                outputs.len()
            } else {
                *num_states
            };
            let symbols = input_mode
                .unwrap_or(InputMode::OpponentLastAction)
                .alphabet_size();
            format!("FSM (s={states}, k={symbols})")
        }
        StrategySpecKind::Ca { k, r, t, .. } => {
            format!("CA (k={k}, r={r}, t={t})")
        }
        StrategySpecKind::OneSidedTm {
            states, symbols, ..
        } => {
            format!("TM (s={states}, k={symbols})")
        }
    }
}

fn render_strategy_table(
    strategies: &[&nit_games::config::StrategySpec],
    width: usize,
    label_style: Style,
    value_style: Style,
    dim_style: Style,
) -> Vec<Line<'static>> {
    let id_header = "id";
    let type_header = "type";
    let summary_header = "summary";
    let summaries: Vec<String> = strategies
        .iter()
        .map(|strategy| strategy_display_name(strategy))
        .collect();

    let mut id_w = strategies
        .iter()
        .map(|strategy| strategy.id.len())
        .max()
        .unwrap_or(id_header.len())
        .max(id_header.len())
        .min(18);
    let mut type_w = strategies
        .iter()
        .map(|strategy| strategy_kind_label(&strategy.kind).len())
        .max()
        .unwrap_or(type_header.len())
        .max(type_header.len())
        .min(6);
    let columns = 3usize;
    let overhead = (columns + 1) + (2 * columns);
    let available = width.saturating_sub(overhead);
    let mut summary_w = summaries
        .iter()
        .map(|summary| summary.len())
        .max()
        .unwrap_or(summary_header.len())
        .max(summary_header.len())
        .max(18);

    let total = id_w + type_w + summary_w;
    if total > available {
        let overflow = total - available;
        let id_shrink = id_w.saturating_sub(id_header.len()).min(overflow);
        id_w -= id_shrink;
        let remaining = overflow.saturating_sub(id_shrink);
        let type_shrink = type_w.saturating_sub(type_header.len()).min(remaining);
        type_w -= type_shrink;
        let remaining = remaining.saturating_sub(type_shrink);
        summary_w = summary_w.saturating_sub(remaining);
    }

    if id_w == 0 {
        id_w = 1;
    }
    if type_w == 0 {
        type_w = 1;
    }
    if summary_w == 0 {
        summary_w = 1;
    }

    let sep = format!(
        "+{}+{}+{}+",
        "-".repeat(id_w + 2),
        "-".repeat(type_w + 2),
        "-".repeat(summary_w + 2)
    );
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));
    lines.push(Line::from(vec![
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(&truncate_text(id_header, id_w), id_w)),
            label_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(
                " {} ",
                center_text(&truncate_text(type_header, type_w), type_w)
            ),
            label_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(
                " {} ",
                center_text(&truncate_text(summary_header, summary_w), summary_w)
            ),
            label_style,
        ),
        Span::styled("|", dim_style),
    ]));
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));

    for (strategy, summary) in strategies.iter().zip(summaries.iter()) {
        let id = truncate_text(&strategy.id, id_w);
        let kind = truncate_text(strategy_kind_label(&strategy.kind), type_w);
        let summary = truncate_text(summary, summary_w);
        lines.push(Line::from(vec![
            Span::styled("|", dim_style),
            Span::styled(format!(" {id:<id_w$} "), value_style),
            Span::styled("|", dim_style),
            Span::styled(format!(" {kind:<type_w$} "), value_style),
            Span::styled("|", dim_style),
            Span::styled(format!(" {summary:<summary_w$} "), value_style),
            Span::styled("|", dim_style),
        ]));
    }

    lines.push(Line::from(Span::styled(sep, dim_style)));
    lines
}

fn payoff_lines(
    payoff: &nit_games::game::PayoffMatrix,
    width: usize,
    value_style: Style,
    dim_style: Style,
    label_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("payoff: ", label_style),
        Span::styled(
            format!(
                "R={} S={} T={} P={}",
                payoff.r, payoff.s, payoff.t, payoff.p
            ),
            value_style,
        ),
    ]));
    lines.push(Line::from(vec![Span::styled("matrix:", label_style)]));
    lines.extend(render_payoff_matrix(
        payoff,
        width,
        value_style,
        dim_style,
        label_style,
    ));
    lines
}

#[allow(clippy::too_many_arguments)]
fn last_run_lines(
    state: &AppState,
    header_style: Style,
    label_style: Style,
    value_style: Style,
    dim_style: Style,
    file_dim_style: Style,
    number_style: Style,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled("Last Run", header_style)));
    if let Some(run) = state.games.last_run.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("timestamp: ", label_style),
            Span::styled(run.timestamp.clone(), value_style),
            Span::raw("  "),
            Span::styled("seed: ", label_style),
            Span::styled(run.seed.to_string(), value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("run_id: ", label_style),
            Span::styled(run.run_id.clone(), value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("accelerator: ", label_style),
            Span::styled(format_runtime_accelerator(&run.runtime), value_style),
        ]));
        if let Some(reason) = run.runtime.metal_fallback_reason.as_ref() {
            lines.push(Line::from(vec![
                Span::styled("accel_note: ", label_style),
                Span::styled(reason.clone(), dim_style),
            ]));
        }
        if let Some(key) = run.runtime.metal_policy_cache_key.as_ref() {
            lines.push(Line::from(vec![
                Span::styled("accel_key: ", label_style),
                Span::styled(key.clone(), dim_style),
            ]));
        }
        if let Some(path) = run.runtime.metal_policy_cache_path.as_ref() {
            lines.push(Line::from(vec![
                Span::styled("accel_cache: ", label_style),
                Span::styled(path.clone(), file_dim_style),
            ]));
        }
        lines.extend(render_last_run_table(
            run,
            width,
            label_style,
            value_style,
            dim_style,
            number_style,
        ));
        let summary_path = run
            .paths
            .summary
            .as_ref()
            .or(state.games.last_run_path.as_ref());
        let history_path = run
            .paths
            .history
            .as_ref()
            .or(state.games.last_history_path.as_ref());
        let event_path = run
            .paths
            .events
            .as_ref()
            .or(state.games.last_event_path.as_ref());
        if let Some(path) = summary_path {
            lines.push(Line::from(vec![
                Span::styled("summary: ", label_style),
                Span::styled(path.clone(), file_dim_style),
            ]));
        }
        if let Some(path) = history_path.or(run.history_log.as_ref()) {
            lines.push(Line::from(vec![
                Span::styled("history: ", label_style),
                Span::styled(path.clone(), file_dim_style),
            ]));
        }
        if let Some(path) = event_path.or(run.event_log.as_ref()) {
            lines.push(Line::from(vec![
                Span::styled("events: ", label_style),
                Span::styled(path.clone(), file_dim_style),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No completed runs yet.",
            dim_style,
        )));
    }
    lines
}

fn format_runtime_accelerator(runtime: &nit_games::RuntimeAcceleratorStats) -> String {
    let backend = match runtime.backend {
        nit_games::RuntimeAcceleratorBackend::Metal => "metal",
        nit_games::RuntimeAcceleratorBackend::Cpu => "cpu",
        nit_games::RuntimeAcceleratorBackend::None => match runtime.requested {
            nit_games::AcceleratorMode::Cpu => "cpu?",
            nit_games::AcceleratorMode::Metal => "metal?",
            nit_games::AcceleratorMode::Auto => "auto",
        },
    };
    let mut parts = vec![backend.to_string()];
    if runtime.metal_matches > 0 {
        parts.push(format!("gpu {}", runtime.metal_matches));
    }
    if runtime.cpu_matches > 0 {
        parts.push(format!("cpu {}", runtime.cpu_matches));
    }
    if runtime.metal_fallbacks > 0 {
        parts.push(format!("fallback {}", runtime.metal_fallbacks));
    }
    if let (Some(batch), Some(inflight)) = (
        runtime.metal_matches_per_batch,
        runtime.metal_inflight_batches,
    ) {
        let label = runtime
            .metal_policy_source_label()
            .map(|source| format!("policy {}x{} {}", batch, inflight, source))
            .unwrap_or_else(|| format!("policy {}x{}", batch, inflight));
        parts.push(label);
    }
    parts.join(", ")
}

#[allow(clippy::too_many_arguments)]
fn render_last_run_table(
    run: &nit_games::output::RunSummary,
    width: usize,
    label_style: Style,
    value_style: Style,
    dim_style: Style,
    number_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut rank_w = "#".len();
    let mut id_w = "id".len();
    let score_header = score_column_label(&run.config);
    let total_header = total_payoff_column_label(&run.config);
    let mut score_w = score_header.len();
    let mut total_w = total_header.len();

    for (idx, entry) in run.results.ranking.iter().take(5).enumerate() {
        rank_w = rank_w.max((idx + 1).to_string().len());
        id_w = id_w.max(entry.id.len());
        score_w = score_w.max(
            entry
                .formatted_score(
                    run.config.engine.score_aggregation,
                    run.config.engine.complexity_cost.enabled,
                )
                .len(),
        );
        total_w = total_w.max(
            entry
                .formatted_total_payoff(
                    run.config.engine.score_aggregation,
                    run.config.engine.complexity_cost.enabled,
                )
                .len(),
        );
    }

    let columns = 4usize;
    let overhead = (columns + 1) + (2 * columns);
    let fixed = rank_w + score_w + total_w;
    let max_id = width.saturating_sub(overhead + fixed).max(1);
    id_w = id_w.min(max_id);

    let sep = format!(
        "+{}+{}+{}+{}+",
        "-".repeat(rank_w + 2),
        "-".repeat(id_w + 2),
        "-".repeat(score_w + 2),
        "-".repeat(total_w + 2)
    );
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));
    lines.push(Line::from(vec![
        Span::styled("|", dim_style),
        Span::styled(format!(" {} ", center_text("#", rank_w)), label_style),
        Span::styled("|", dim_style),
        Span::styled(format!(" {} ", center_text("id", id_w)), label_style),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(score_header, score_w)),
            label_style,
        ),
        Span::styled("|", dim_style),
        Span::styled(
            format!(" {} ", center_text(total_header, total_w)),
            label_style,
        ),
        Span::styled("|", dim_style),
    ]));
    lines.push(Line::from(Span::styled(sep.clone(), dim_style)));

    for (idx, entry) in run.results.ranking.iter().take(5).enumerate() {
        let rank = (idx + 1).to_string();
        let id = truncate_text(&entry.id, id_w);
        let score = entry.formatted_score(
            run.config.engine.score_aggregation,
            run.config.engine.complexity_cost.enabled,
        );
        let total = entry.formatted_total_payoff(
            run.config.engine.score_aggregation,
            run.config.engine.complexity_cost.enabled,
        );

        let mut spans = Vec::new();
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {rank:>rank_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {id:<id_w$} "), value_style));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {score:>score_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        spans.push(Span::styled(format!(" {total:>total_w$} "), number_style));
        spans.push(Span::styled("|", dim_style));
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(Span::styled(sep, dim_style)));
    lines
}

fn score_column_label(config: &nit_games::config::NormalizedConfig) -> &'static str {
    match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean => "mean",
        nit_games::ScoreAggregation::Total => "score",
    }
}

fn total_payoff_column_label(config: &nit_games::config::NormalizedConfig) -> &'static str {
    match config.engine.score_aggregation {
        nit_games::ScoreAggregation::Mean | nit_games::ScoreAggregation::Total => "payoff",
    }
}

fn payoff_panel_width(payoff: &nit_games::game::PayoffMatrix) -> usize {
    let payoff_summary = format!(
        "payoff: R={} S={} T={} P={}",
        payoff.r, payoff.s, payoff.t, payoff.p
    )
    .len();
    let matrix_width = payoff_matrix_width(payoff);
    payoff_summary.max(matrix_width)
}

fn payoff_matrix_width(payoff: &nit_games::game::PayoffMatrix) -> usize {
    let cc = format!("({},{})", payoff.matrix[0][0][0], payoff.matrix[0][0][1]);
    let cd = format!("({},{})", payoff.matrix[0][1][0], payoff.matrix[0][1][1]);
    let dc = format!("({},{})", payoff.matrix[1][0][0], payoff.matrix[1][0][1]);
    let dd = format!("({},{})", payoff.matrix[1][1][0], payoff.matrix[1][1][1]);
    let cell_width = [cc.len(), cd.len(), dc.len(), dd.len(), 1]
        .into_iter()
        .max()
        .unwrap_or(1)
        + 2;
    let row_label_width = 3;
    row_label_width + cell_width * 2 + 4
}

fn render_payoff_matrix(
    payoff: &nit_games::game::PayoffMatrix,
    width: usize,
    value_style: Style,
    dim_style: Style,
    label_style: Style,
) -> Vec<Line<'static>> {
    let cc = format!("({},{})", payoff.matrix[0][0][0], payoff.matrix[0][0][1]);
    let cd = format!("({},{})", payoff.matrix[0][1][0], payoff.matrix[0][1][1]);
    let dc = format!("({},{})", payoff.matrix[1][0][0], payoff.matrix[1][0][1]);
    let dd = format!("({},{})", payoff.matrix[1][1][0], payoff.matrix[1][1][1]);
    let cell_width = [cc.len(), cd.len(), dc.len(), dd.len(), 1]
        .into_iter()
        .max()
        .unwrap_or(1)
        + 2;
    let row_label_width = 3;

    let top = format!(
        "+{}+{}+{}+",
        "-".repeat(row_label_width),
        "-".repeat(cell_width),
        "-".repeat(cell_width)
    );
    let header = format!(
        "|{}|{}|{}|",
        center_text("", row_label_width),
        center_text("C", cell_width),
        center_text("D", cell_width)
    );
    let row_c = format!(
        "|{}|{}|{}|",
        center_text("C", row_label_width),
        center_text(&cc, cell_width),
        center_text(&cd, cell_width)
    );
    let row_d = format!(
        "|{}|{}|{}|",
        center_text("D", row_label_width),
        center_text(&dc, cell_width),
        center_text(&dd, cell_width)
    );

    vec![
        centered_line(&top, width, dim_style),
        centered_header_line(&header, width, dim_style, label_style),
        centered_line(&top, width, dim_style),
        centered_line(&row_c, width, value_style),
        centered_line(&row_d, width, value_style),
        centered_line(&top, width, dim_style),
    ]
}

fn center_text(text: &str, width: usize) -> String {
    if text.len() >= width {
        return text.to_string();
    }
    let pad_total = width - text.len();
    let pad_left = pad_total / 2;
    let pad_right = pad_total - pad_left;
    format!("{}{}{}", " ".repeat(pad_left), text, " ".repeat(pad_right))
}

fn truncate_text(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }
    let mut out: String = text.chars().take(width - 3).collect();
    out.push_str("...");
    out
}

fn centered_line(line: &str, width: usize, style: Style) -> Line<'static> {
    let pad = center_pad(width, line.len());
    Line::from(vec![
        Span::styled(pad, style),
        Span::styled(line.to_string(), style),
    ])
}

fn centered_header_line(
    line: &str,
    width: usize,
    border_style: Style,
    header_style: Style,
) -> Line<'static> {
    let pad = center_pad(width, line.len());
    let mut spans = Vec::new();
    spans.push(Span::styled(pad, border_style));
    let mut buf = String::new();
    let mut header_spans = Vec::new();
    for ch in line.chars() {
        match ch {
            'C' | 'D' => {
                if !buf.is_empty() {
                    header_spans.push(Span::styled(buf.clone(), border_style));
                    buf.clear();
                }
                header_spans.push(Span::styled(ch.to_string(), header_style));
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        header_spans.push(Span::styled(buf, border_style));
    }
    spans.extend(header_spans);
    Line::from(spans)
}

fn center_pad(width: usize, line_len: usize) -> String {
    if width <= line_len {
        return String::new();
    }
    let pad = (width - line_len) / 2;
    " ".repeat(pad)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn last_run_table_shows_total_payoff_column_without_wld() {
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
        let requested_accelerator = config.engine.accelerator;

        let mut runtime = nit_games::RuntimeAcceleratorStats::new(requested_accelerator);
        runtime.note_metal_policy(
            131_072,
            4,
            nit_games::BatchPolicySource::Cached,
            Some("apple_m4_max_demo".into()),
            Some("/tmp/apple_m4_max_demo_v1.json".into()),
        );

        let run = nit_games::output::RunSummary {
            schema_version: nit_games::output::RUN_SUMMARY_SCHEMA_VERSION,
            timestamp: "2026-03-08T00:00:00Z".into(),
            run_id: "run".into(),
            seed: 42,
            config_text: String::new(),
            config,
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
            results: nit_games::output::TournamentResults {
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
            },
            event_log: None,
            history_log: None,
            runtime,
            run_dir: None,
        };

        let lines = render_last_run_table(
            &run,
            120,
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
        );

        assert!(lines.iter().any(|line| line_text(line).contains("mean")));
        assert!(lines.iter().any(|line| line_text(line).contains("payoff")));
        assert!(lines.iter().any(|line| line_text(line).contains(" -2 ")));
        assert!(lines.iter().all(|line| !line_text(line).contains("W-L-D")));
    }

    #[test]
    fn runtime_accelerator_formatter_includes_gpu_counts() {
        let mut runtime =
            nit_games::RuntimeAcceleratorStats::new(nit_games::AcceleratorMode::Metal);
        runtime.note_metal_batches(2, 32);
        runtime.note_cpu_matches(1);
        runtime.note_metal_policy(
            131_072,
            4,
            nit_games::BatchPolicySource::Cached,
            Some("apple_m4_max_demo".into()),
            Some("/tmp/apple_m4_max_demo_v1.json".into()),
        );

        let text = format_runtime_accelerator(&runtime);
        assert!(text.contains("metal"));
        assert!(text.contains("gpu 32"));
        assert!(text.contains("cpu 1"));
        assert!(text.contains("policy 131072x4 cached"));
    }

    #[test]
    fn fsm_strategy_display_name_uses_symbol_count() {
        let opponent = strategy_display_name_from_kind(&StrategySpecKind::Fsm {
            outputs: vec![],
            start_state: 0,
            transitions: vec![],
            num_states: 1,
            input_mode: Some(InputMode::OpponentLastAction),
            index: None,
        });
        let joint = strategy_display_name_from_kind(&StrategySpecKind::Fsm {
            outputs: vec![],
            start_state: 0,
            transitions: vec![],
            num_states: 1,
            input_mode: Some(InputMode::JointLastAction),
            index: None,
        });

        assert_eq!(opponent, "FSM (s=1, k=2)");
        assert_eq!(joint, "FSM (s=1, k=4)");
    }

    #[test]
    fn tm_strategy_display_name_uses_short_state_and_symbol_labels() {
        let tm = strategy_display_name_from_kind(&StrategySpecKind::OneSidedTm {
            states: 2,
            symbols: 3,
            start_state: 1,
            blank: 0,
            fallback_symbol: None,
            max_steps_per_round: 8,
            input_mode: InputMode::OpponentLastAction,
            output_map: vec![],
            transitions: vec![],
            rule_code: None,
        });

        assert_eq!(tm, "TM (s=2, k=3)");
    }

    #[test]
    fn strategy_table_renders_bordered_summary_layout() {
        let strategies = vec![
            nit_games::config::StrategySpec {
                id: "fsm_allc".into(),
                name: None,
                kind: StrategySpecKind::Fsm {
                    num_states: 1,
                    start_state: 0,
                    outputs: vec![],
                    input_mode: Some(InputMode::OpponentLastAction),
                    transitions: vec![],
                    index: None,
                },
            },
            nit_games::config::StrategySpec {
                id: "tm_timeout".into(),
                name: None,
                kind: StrategySpecKind::OneSidedTm {
                    states: 1,
                    symbols: 2,
                    start_state: 1,
                    blank: 0,
                    fallback_symbol: None,
                    max_steps_per_round: 8,
                    input_mode: InputMode::OpponentLastAction,
                    output_map: vec![],
                    transitions: vec![],
                    rule_code: None,
                },
            },
        ];
        let refs: Vec<_> = strategies.iter().collect();

        let lines = render_strategy_table(
            &refs,
            54,
            Style::default(),
            Style::default(),
            Style::default(),
        );
        let rendered: Vec<String> = lines.iter().map(line_text).collect();

        assert!(rendered[0].starts_with('+'));
        assert!(rendered[0].chars().count() < 54);
        assert!(rendered[1].starts_with('|'));
        assert!(rendered[1].contains("id"));
        assert!(rendered[1].contains("summary"));
        assert!(rendered.iter().any(|line| line.contains("fsm_allc")));
        assert!(rendered.iter().any(|line| line.contains("FSM (s=1, k=2)")));
        assert!(rendered.iter().any(|line| line.contains("TM (s=1, k=2)")));
    }

    #[test]
    fn payoff_lines_omit_payoff_legend_copy() {
        let payoff = nit_games::game::PayoffMatrix::default_pd();
        let lines = payoff_lines(
            &payoff,
            60,
            Style::default(),
            Style::default(),
            Style::default(),
        );
        let rendered: Vec<String> = lines.iter().map(line_text).collect();

        assert!(rendered.iter().any(|line| line.contains("payoff: R=")));
        assert!(rendered.iter().any(|line| line.contains("matrix:")));
        assert!(rendered.iter().all(|line| !line.contains("reward (C,C)")));
        assert!(rendered.iter().all(|line| !line.contains("sucker (C,D)")));
        assert!(rendered
            .iter()
            .all(|line| !line.contains("temptation (D,C)")));
        assert!(rendered
            .iter()
            .all(|line| !line.contains("punishment (D,D)")));
    }

    #[test]
    fn layout_for_config_keeps_side_panel_width_stable_across_last_run_content() {
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
        let state = AppState::new(
            std::env::temp_dir(),
            nit_core::Buffer::empty("x", None),
            nit_core::Buffer::empty("n", None),
        );
        let empty_layout = layout_for_config(
            Rect {
                x: 0,
                y: 0,
                width: 90,
                height: 30,
            },
            &state,
            Some(&config),
        );
        let empty_side = empty_layout.side.expect("empty side panel");

        let mut state_with_run = AppState::new(
            std::env::temp_dir(),
            nit_core::Buffer::empty("x", None),
            nit_core::Buffer::empty("n", None),
        );
        state_with_run.games.last_run = Some(nit_games::output::RunSummary {
            schema_version: nit_games::output::RUN_SUMMARY_SCHEMA_VERSION,
            timestamp: "2026-03-11T23:00:28.511548Z".into(),
            run_id: "82d85202af92c04e".into(),
            seed: 42,
            config_text: String::new(),
            config: config.clone(),
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
            results: nit_games::output::TournamentResults {
                ranking: vec![nit_games::output::StrategyResult {
                    id: "fsm_alld".into(),
                    name: None,
                    total_payoff: -1690,
                    average_payoff: -0.884,
                    adjusted_total_payoff: Some(-1690.0),
                    adjusted_average_payoff: Some(-0.884),
                    matches: 1,
                    wins: 0,
                    losses: 0,
                    draws: 1,
                    crashed: false,
                    crash_count: 0,
                    tm_metrics: None,
                }],
                pairwise: Vec::new(),
                dominance: Vec::new(),
            },
            event_log: None,
            history_log: None,
            runtime: nit_games::RuntimeAcceleratorStats::default(),
            run_dir: None,
        });

        let layout = layout_for_config(
            Rect {
                x: 0,
                y: 0,
                width: 90,
                height: 30,
            },
            &state_with_run,
            Some(&config),
        );

        let side = layout.side.expect("side panel");
        assert!(layout.show_payoff_side);
        assert_eq!(side.width, empty_side.width);
    }

    #[test]
    fn layout_for_config_keeps_empty_last_run_panel_wide_enough() {
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
        let state = AppState::new(
            std::env::temp_dir(),
            nit_core::Buffer::empty("x", None),
            nit_core::Buffer::empty("n", None),
        );

        let layout = layout_for_config(
            Rect {
                x: 0,
                y: 0,
                width: 90,
                height: 30,
            },
            &state,
            Some(&config),
        );

        let side = layout.side.expect("side panel");
        assert!(layout.show_payoff_side);
        assert!(
            side.width as usize >= LAST_RUN_PANEL_TARGET_WIDTH + 2 + LAST_RUN_PANEL_EXTRA_WIDTH
        );
    }
}
