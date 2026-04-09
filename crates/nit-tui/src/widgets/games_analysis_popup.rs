use std::path::Path;

use nit_core::AppState;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;
use nit_core::UiSelectionPane;

const MIN_WIDTH: u16 = 64;
const MIN_HEIGHT: u16 = 18;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen.width.clamp(MIN_WIDTH, 96);
    let height = screen.height.clamp(MIN_HEIGHT, 32);
    (width, height)
}

/// Count the rendered lines without building any `Line`/`Span` structures.
/// Used by the scroll hot path so wheel ticks don't rebuild sparklines,
/// strategy tables, or styled spans just to figure out `max_scroll`. Must
/// stay in lock-step with `build_lines` below.
pub fn line_count(state: &AppState) -> usize {
    // status line
    let mut count = 1usize;
    if state.games.analysis.source_path.is_some() {
        count += 1;
    }
    if state.games.analysis.last_error.is_some() {
        count += 1;
    }
    if state.games.analysis.running && state.games.analysis.summary.is_none() {
        count += 2;
    }
    if let Some(summary) = state.games.analysis.summary.as_ref() {
        // blank + matches/rounds + rounds/match samples
        count += 3;
        // blank + "Outputs" header + 4 path rows
        count += 6;
        // blank + "Strategy cooperation" header
        count += 2;
        count += summary.strategies.len();
        if let Some(preview) = state.games.analysis.preview.as_ref() {
            // blank + "Random match trajectories" header
            count += 2;
            if preview.trajectories.is_empty() {
                count += 1;
            } else {
                // title + A: + B: per trajectory
                count += preview.trajectories.len() * 3;
            }
        }
    }
    count
}

pub fn build_lines(state: &AppState, theme: &Theme, inner_width: u16) -> Vec<Line<'static>> {
    let header_style = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let number_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let warn_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);

    let max_width = inner_width.max(1) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Title is already in the popup border; avoid duplicate header line.
    lines.push(Line::from(vec![
        Span::styled("status: ", label_style),
        Span::styled(
            if state.games.analysis.running {
                "RUNNING"
            } else if state.games.analysis.last_error.is_some() {
                "ERROR"
            } else if state.games.analysis.summary.is_some() {
                "DONE"
            } else {
                "IDLE"
            },
            if state.games.analysis.last_error.is_some() {
                warn_style
            } else {
                number_style
            },
        ),
    ]));

    if let Some(path) = state.games.analysis.source_path.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("source: ", label_style),
            Span::styled(short_path(path, max_width), value_style),
        ]));
    }

    if let Some(err) = state.games.analysis.last_error.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("error: ", warn_style),
            Span::styled(trim_to_width(err, max_width), value_style),
        ]));
    }

    if state.games.analysis.running && state.games.analysis.summary.is_none() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                trim_to_width("Analyzing history log...", max_width),
                value_style,
            ),
            Span::styled(" (Esc to close)", dim_style),
        ]));
    }

    if let Some(summary) = state.games.analysis.summary.as_ref() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("matches: ", label_style),
            Span::styled(summary.total_matches.to_string(), number_style),
            Span::styled("  rounds: ", label_style),
            Span::styled(summary.total_rounds.to_string(), number_style),
            Span::styled("  tail: ", label_style),
            Span::styled(summary.tail_rounds.to_string(), number_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("rounds/match: ", label_style),
            Span::styled(summary.min_rounds.to_string(), number_style),
            Span::styled("..", dim_style),
            Span::styled(summary.max_rounds.to_string(), number_style),
            Span::styled("  samples: ", label_style),
            Span::styled(summary.trajectory_samples.to_string(), number_style),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Outputs", header_style)));
        lines.push(Line::from(vec![
            Span::styled("summary: ", label_style),
            Span::styled(short_path(&summary.paths.summary, max_width), value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("matches: ", label_style),
            Span::styled(
                short_path(&summary.paths.matches_csv, max_width),
                value_style,
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("strategies: ", label_style),
            Span::styled(
                short_path(&summary.paths.strategies_csv, max_width),
                value_style,
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("trajectories: ", label_style),
            Span::styled(
                short_path(&summary.paths.trajectories_csv, max_width),
                value_style,
            ),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Strategy cooperation (overall / tail)",
            header_style,
        )));
        let id_width = 14usize.min(max_width / 3).max(6);
        for (idx, strat) in summary.strategies.iter().enumerate() {
            let id = trim_to_width(&strat.id, id_width);
            let coop = strat.coop_rate * 100.0;
            let tail = strat.tail_coop_rate * 100.0;
            let rounds = strat.rounds;
            lines.push(Line::from(vec![
                Span::styled(format!("{:>2} ", idx + 1), dim_style),
                Span::styled(format!("{id:<id_width$}"), value_style),
                Span::styled(" ", dim_style),
                Span::styled(format!("{coop:>6.2}%"), number_style),
                Span::styled(" / ", dim_style),
                Span::styled(format!("{tail:>6.2}%"), number_style),
                Span::styled("  r=", dim_style),
                Span::styled(rounds.to_string(), value_style),
            ]));
        }

        if let Some(preview) = state.games.analysis.preview.as_ref() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Random match trajectories",
                header_style,
            )));
            if preview.trajectories.is_empty() {
                lines.push(Line::from(Span::styled(
                    "No random matchups detected.",
                    dim_style,
                )));
            } else {
                let plot_width = max_width.saturating_sub(12).max(1);
                for traj in preview.trajectories.iter() {
                    let title = format!("{} vs {}", traj.a, traj.b);
                    lines.push(Line::from(Span::styled(
                        trim_to_width(&title, max_width),
                        value_style,
                    )));
                    let a_plot = sparkline(&traj.a_rates, plot_width);
                    let b_plot = sparkline(&traj.b_rates, plot_width);
                    lines.push(Line::from(vec![
                        Span::styled("A: ", label_style),
                        Span::styled(a_plot, number_style),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("B: ", label_style),
                        Span::styled(b_plot, number_style),
                    ]));
                }
            }
        }
    }

    lines
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if !state.games.analysis.open {
        return;
    }

    frame.render_widget(Clear, area);

    let border_style = Style::default().fg(theme.border_focused);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            " GAMES ANALYSIS ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = build_lines(state, theme, inner.width);
    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = state.games.analysis.scroll_offset.min(max_scroll);
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
    let visible = apply_ui_selection(
        visible,
        state.ui_selection.as_ref(),
        UiSelectionPane::GamesAnalysisPopup,
        theme.selection_bg,
        scroll,
    );
    let paragraph = Paragraph::new(visible)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, inner);
}

fn short_path(path: &str, max_width: usize) -> String {
    let name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    trim_to_width(name, max_width)
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars().take(max_width) {
        out.push(ch);
    }
    out
}

fn sparkline(values: &[f64], width: usize) -> String {
    const LEVELS: &[u8] = b" .:-=+*#%@";
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let samples = width.min(values.len()).max(1);
    let mut line = String::with_capacity(samples);
    for i in 0..samples {
        let start = i * values.len() / samples;
        let end = ((i + 1) * values.len() / samples).max(start + 1);
        let slice = &values[start..end.min(values.len())];
        let avg = slice.iter().copied().sum::<f64>() / slice.len().max(1) as f64;
        let idx = (avg.clamp(0.0, 1.0) * (LEVELS.len() as f64 - 1.0)).round() as usize;
        line.push(LEVELS[idx] as char);
    }
    line
}
