use nit_core::{AppState, PaneId};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState, theme: &Theme) {
    let focused = state.focus == PaneId::JobOutput;
    let border_style = if focused {
        Style::default().fg(theme.border_focused)
    } else {
        Style::default().fg(theme.border)
    };
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            "JOB OUTPUT  [ PAUSE ] [ CLEAR ]",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);

    // Progress gauge
    let gauge = Gauge::default()
        .block(Block::default().style(Style::default().bg(theme.background)))
        .gauge_style(
            Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .ratio(state.job.progress as f64)
        .label(Span::styled(
            if state.job.paused {
                "PAUSED"
            } else {
                "RUNNING"
            },
            Style::default().fg(theme.foreground),
        ));
    frame.render_widget(block, area);
    frame.render_widget(gauge, chunks[0]);

    // Logs
    let height = chunks[1].height as usize;
    let logs: Vec<_> = state.logs.iter().rev().take(height).cloned().collect();
    let mut lines: Vec<Line> = Vec::new();
    for line in logs.into_iter().rev() {
        lines.push(Line::from(Span::styled(
            line,
            Style::default().fg(theme.foreground),
        )));
    }

    let paragraph =
        Paragraph::new(lines).style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(paragraph, chunks[1]);
}
