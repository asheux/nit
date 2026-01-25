use nit_core::{AppState, PaneId};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState, theme: &Theme) {
    let hints = vec![
        hint("Ctrl+Q", "Quit", theme),
        hint("Ctrl+S", "Save", theme),
        hint("Tab", "Focus", theme),
        hint("Ctrl+HJKL", "Pane", theme),
        hint("F1", "Help", theme),
        hint("Ctrl+Shift+L", "Clear Logs", theme),
        hint("Ctrl+Shift+H", "Syntax", theme),
        hint("Ctrl+R", "Seed", theme),
        hint("Ctrl+A", "Apply", theme),
    ];
    let focus = format!("FOCUS: {}", focus_name(state.focus));
    let mut spans = Vec::new();
    for h in hints {
        spans.push(h);
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        focus,
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    ));

    if let Some(status) = &state.status {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("STATUS: {status}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    let para = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .block(block);
    frame.render_widget(para, area);
}

fn hint(key: &str, label: &str, theme: &Theme) -> Span<'static> {
    Span::styled(
        format!("[ {key} {label} ]"),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )
}

fn focus_name(pane: PaneId) -> &'static str {
    pane.title()
}
