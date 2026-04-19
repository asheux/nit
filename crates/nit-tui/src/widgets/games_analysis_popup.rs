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
use crate::widgets::text_utils::trim_to_width;
use nit_core::UiSelectionPane;

const MIN_WIDTH: u16 = 64;
const MAX_WIDTH: u16 = 96;
const MIN_HEIGHT: u16 = 18;
const MAX_HEIGHT: u16 = 32;

/// Desired `(width, height)` for the analysis popup, clamped to
/// MIN/MAX_* and never exceeding the screen.
pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen.width.clamp(MIN_WIDTH, MAX_WIDTH);
    let height = screen.height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    (width, height)
}

/// Count rendered lines without allocating any `Line`/`Span` — the scroll
/// hot path uses this so wheel ticks don't rebuild sparklines or strategy
/// tables just to compute `max_scroll`. Must mirror `build_lines`.
pub fn line_count(state: &AppState) -> usize {
    let a = &state.games.analysis;
    let mut count = 1usize; // status line
    if a.source_path.is_some() {
        count += 1;
    }
    if a.last_error.is_some() {
        count += 1;
    }
    if a.running && a.summary.is_none() {
        count += 2;
    }
    if let Some(summary) = a.summary.as_ref() {
        count += 3; // blank + matches/rounds + samples
        count += 6; // blank + "Outputs" header + 4 path rows
        count += 2; // blank + "Strategy cooperation" header
        count += summary.strategies.len();
        if let Some(preview) = a.preview.as_ref() {
            count += 2; // blank + "Random match trajectories" header
            if preview.trajectories.is_empty() {
                count += 1;
            } else {
                count += preview.trajectories.len() * 3;
            }
        }
    }
    count
}

struct Styles {
    header: Style,
    label: Style,
    value: Style,
    dim: Style,
    number: Style,
    warn: Style,
}

impl Styles {
    fn new(theme: &Theme) -> Self {
        Self {
            header: Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
            label: Style::default().fg(theme.title).add_modifier(Modifier::DIM),
            value: Style::default().fg(theme.foreground),
            dim: Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
            number: Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            warn: Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        }
    }
}

pub fn build_lines(state: &AppState, theme: &Theme, inner_width: u16) -> Vec<Line<'static>> {
    let styles = Styles::new(theme);
    let max_width = inner_width.max(1) as usize;
    let a = &state.games.analysis;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(status_line(a, &styles));

    if let Some(path) = a.source_path.as_deref() {
        lines.push(kv_line("source: ", &short_path(path, max_width), &styles));
    }
    if let Some(err) = a.last_error.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("error: ", styles.warn),
            Span::styled(trim_to_width(err, max_width), styles.value),
        ]));
    }
    if a.running && a.summary.is_none() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                trim_to_width("Analyzing history log...", max_width),
                styles.value,
            ),
            Span::styled(" (Esc to close)", styles.dim),
        ]));
    }

    if let Some(summary) = a.summary.as_ref() {
        append_summary_section(&mut lines, summary, &styles, max_width);
        if let Some(preview) = a.preview.as_ref() {
            append_preview_section(&mut lines, preview, &styles, max_width);
        }
    }

    lines
}

fn status_line(a: &nit_core::GamesAnalysisState, styles: &Styles) -> Line<'static> {
    let (text, style) = status_label(a, styles);
    Line::from(vec![
        Span::styled("status: ", styles.label),
        Span::styled(text, style),
    ])
}

fn status_label(a: &nit_core::GamesAnalysisState, styles: &Styles) -> (&'static str, Style) {
    if a.running {
        ("RUNNING", styles.number)
    } else if a.last_error.is_some() {
        ("ERROR", styles.warn)
    } else if a.summary.is_some() {
        ("DONE", styles.number)
    } else {
        ("IDLE", styles.number)
    }
}

fn kv_line(label: &'static str, value: &str, styles: &Styles) -> Line<'static> {
    Line::from(vec![
        Span::styled(label, styles.label),
        Span::styled(value.to_string(), styles.value),
    ])
}

fn append_summary_section(
    lines: &mut Vec<Line<'static>>,
    summary: &nit_games::analysis::HistoryAnalysisSummary,
    styles: &Styles,
    max_width: usize,
) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("matches: ", styles.label),
        Span::styled(summary.total_matches.to_string(), styles.number),
        Span::styled("  rounds: ", styles.label),
        Span::styled(summary.total_rounds.to_string(), styles.number),
        Span::styled("  tail: ", styles.label),
        Span::styled(summary.tail_rounds.to_string(), styles.number),
    ]));
    lines.push(Line::from(vec![
        Span::styled("rounds/match: ", styles.label),
        Span::styled(summary.min_rounds.to_string(), styles.number),
        Span::styled("..", styles.dim),
        Span::styled(summary.max_rounds.to_string(), styles.number),
        Span::styled("  samples: ", styles.label),
        Span::styled(summary.trajectory_samples.to_string(), styles.number),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Outputs", styles.header)));
    for (label, path) in [
        ("summary: ", &summary.paths.summary),
        ("matches: ", &summary.paths.matches_csv),
        ("strategies: ", &summary.paths.strategies_csv),
        ("trajectories: ", &summary.paths.trajectories_csv),
    ] {
        lines.push(kv_line(label, &short_path(path, max_width), styles));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Strategy cooperation (overall / tail)",
        styles.header,
    )));
    let id_width = 14usize.min(max_width / 3).max(6);
    for (idx, strat) in summary.strategies.iter().enumerate() {
        let id = trim_to_width(&strat.id, id_width);
        let coop = strat.coop_rate * 100.0;
        let tail = strat.tail_coop_rate * 100.0;
        lines.push(Line::from(vec![
            Span::styled(format!("{:>2} ", idx + 1), styles.dim),
            Span::styled(format!("{id:<id_width$}"), styles.value),
            Span::styled(" ", styles.dim),
            Span::styled(format!("{coop:>6.2}%"), styles.number),
            Span::styled(" / ", styles.dim),
            Span::styled(format!("{tail:>6.2}%"), styles.number),
            Span::styled("  r=", styles.dim),
            Span::styled(strat.rounds.to_string(), styles.value),
        ]));
    }
}

fn append_preview_section(
    lines: &mut Vec<Line<'static>>,
    preview: &nit_games::analysis::HistoryAnalysisPreview,
    styles: &Styles,
    max_width: usize,
) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Random match trajectories",
        styles.header,
    )));
    if preview.trajectories.is_empty() {
        lines.push(Line::from(Span::styled(
            "No random matchups detected.",
            styles.dim,
        )));
        return;
    }
    let plot_width = max_width.saturating_sub(12).max(1);
    for traj in preview.trajectories.iter() {
        let title = format!("{} vs {}", traj.a, traj.b);
        lines.push(Line::from(Span::styled(
            trim_to_width(&title, max_width),
            styles.value,
        )));
        lines.push(Line::from(vec![
            Span::styled("A: ", styles.label),
            Span::styled(sparkline(&traj.a_rates, plot_width), styles.number),
        ]));
        lines.push(Line::from(vec![
            Span::styled("B: ", styles.label),
            Span::styled(sparkline(&traj.b_rates, plot_width), styles.number),
        ]));
    }
}

/// Paint the analysis popup; no-op when `state.games.analysis.open` is false.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if !state.games.analysis.open {
        return;
    }
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
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
