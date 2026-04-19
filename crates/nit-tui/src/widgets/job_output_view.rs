use nit_core::{AppState, PaneId, UiSelectionPane};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge, Paragraph},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

const PANE_TITLE: &str = "JOB OUTPUT  [ Space/Ctrl+Space/F6 ] [ Ctrl+L ]";
const GAUGE_ROW_HEIGHT: u16 = 1;

/// Render the Job Output pane: a one-row progress gauge on top, scrollable log
/// tail beneath. Log lines are colorized by keyword (ERROR/WARN/…) via
/// [`log_style`]. Scroll offset is clamped against the log buffer length.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let focused = state.focus == PaneId::JobOutput;
    let block = build_block(focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(GAUGE_ROW_HEIGHT), Constraint::Min(1)])
        .split(inner);

    frame.render_widget(build_gauge(state, theme), chunks[0]);
    render_logs(frame, chunks[1], state, theme);
}

fn build_block(focused: bool, theme: &Theme) -> Block<'static> {
    let (border_style, border_type, title_color) = if focused {
        (
            Style::default().fg(theme.border_focused),
            BorderType::Thick,
            theme.title_focused,
        )
    } else {
        (
            Style::default().fg(theme.border),
            BorderType::Plain,
            theme.title,
        )
    };
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            PANE_TITLE,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background))
}

fn build_gauge<'a>(state: &AppState, theme: &'a Theme) -> Gauge<'a> {
    let label = if state.job.paused {
        "PAUSED"
    } else {
        "RUNNING"
    };
    Gauge::default()
        .block(Block::default().style(Style::default().bg(theme.background)))
        .gauge_style(
            Style::default()
                .fg(theme.title_focused)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .ratio(state.job.progress as f64)
        .label(Span::styled(label, Style::default().fg(theme.foreground)))
}

fn render_logs(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let height = area.height as usize;
    let total = state.logs.len();
    let max_scroll = total.saturating_sub(height);
    let scroll = state.logs_scroll.min(max_scroll);
    let start = total.saturating_sub(height + scroll);
    let end = total.saturating_sub(scroll);

    let lines: Vec<Line> = state
        .logs
        .iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|line| Line::from(Span::styled(line.clone(), log_style(line, theme))))
        .collect();

    let lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::JobOutput,
        theme.selection_bg,
        start,
    );

    let paragraph =
        Paragraph::new(lines).style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(paragraph, area);
}

fn log_style(line: &str, theme: &Theme) -> Style {
    let upper = line.to_ascii_uppercase();
    let bold_error = || {
        Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD)
    };
    let bold_warning = || {
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    };
    if upper.contains("PANIC") || upper.contains("ERROR") {
        bold_error()
    } else if upper.contains("WARN") {
        bold_warning()
    } else if upper.contains("SECURITY") || upper.contains("SNAPSHOT") {
        Style::default().fg(theme.accent)
    } else if upper.contains("DEBUG") || upper.contains("HEARTBEAT") {
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM)
    } else if upper.contains("CYCLE") || upper.contains("FIXED POINT") || upper.contains("REPEAT") {
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD)
    } else if upper.contains("PAUSED") {
        bold_warning()
    } else if upper.contains("JOB ") {
        Style::default().fg(theme.title)
    } else {
        Style::default().fg(theme.foreground)
    }
}
