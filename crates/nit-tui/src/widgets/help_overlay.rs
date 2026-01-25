use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: Rect, theme: &Theme) {
    let lines = vec![
        Line::from(vec![
            Span::styled("Ctrl+Q", Style::default().fg(theme.accent)),
            Span::raw(" quit (confirm if dirty)"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+S", Style::default().fg(theme.accent)),
            Span::raw(" save"),
        ]),
        Line::from(vec![
            Span::styled("Tab/Shift+Tab", Style::default().fg(theme.accent)),
            Span::raw(" focus panes"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+H/J/K/L", Style::default().fg(theme.accent)),
            Span::raw(" focus panes (left/down/up/right)"),
        ]),
        Line::from(vec![
            Span::styled("Esc", Style::default().fg(theme.accent)),
            Span::raw(" switch to Normal mode"),
        ]),
        Line::from(vec![
            Span::styled("H/J/K/L", Style::default().fg(theme.accent)),
            Span::raw(" move in Normal mode"),
        ]),
        Line::from(vec![
            Span::styled("I", Style::default().fg(theme.accent)),
            Span::raw(" enter Insert mode"),
        ]),
        Line::from(vec![
            Span::styled("o", Style::default().fg(theme.accent)),
            Span::raw(" open line below + Insert (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("JJ", Style::default().fg(theme.accent)),
            Span::raw(" save + Normal (Insert mode)"),
        ]),
        Line::from(vec![
            Span::styled("GG / Shift+G", Style::default().fg(theme.accent)),
            Span::raw(" top / bottom"),
        ]),
        Line::from(vec![
            Span::styled("u", Style::default().fg(theme.accent)),
            Span::raw(" undo (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("$ / %", Style::default().fg(theme.accent)),
            Span::raw(" end / start of line"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+Shift+L", Style::default().fg(theme.accent)),
            Span::raw(" clear logs"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+R", Style::default().fg(theme.accent)),
            Span::raw(" reseed visualizer"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+A", Style::default().fg(theme.accent)),
            Span::raw(" apply visual variant"),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            "HELP",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.background).fg(theme.foreground));

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1)])
        .split(block.inner(area))[0];

    let para =
        Paragraph::new(lines).style(Style::default().bg(theme.background).fg(theme.foreground));

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}
