use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_sub(4)).min(80).max(30);
    let height = (screen.height.saturating_sub(6)).min(24).max(10);
    (width, height)
}

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
            Span::styled("F1 / ?", Style::default().fg(theme.accent)),
            Span::raw(" toggle help"),
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
            Span::styled("a", Style::default().fg(theme.accent)),
            Span::raw(" append + Insert (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("v", Style::default().fg(theme.accent)),
            Span::raw(" Visual mode (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("o", Style::default().fg(theme.accent)),
            Span::raw(" open line below + Insert (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("Shift+O", Style::default().fg(theme.accent)),
            Span::raw(" open line above + Insert (Normal mode)"),
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
            Span::styled("Shift+R", Style::default().fg(theme.accent)),
            Span::raw(" redo (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("e / b", Style::default().fg(theme.accent)),
            Span::raw(" word end / word back (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("y", Style::default().fg(theme.accent)),
            Span::raw(" yank selection (Visual mode)"),
        ]),
        Line::from(vec![
            Span::styled("yy", Style::default().fg(theme.accent)),
            Span::raw(" yank line (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("d", Style::default().fg(theme.accent)),
            Span::raw(" delete selection (Visual mode)"),
        ]),
        Line::from(vec![
            Span::styled("p", Style::default().fg(theme.accent)),
            Span::raw(" paste (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("Shift+P", Style::default().fg(theme.accent)),
            Span::raw(" paste above (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("dd", Style::default().fg(theme.accent)),
            Span::raw(" delete line (Normal mode)"),
        ]),
        Line::from(vec![
            Span::styled("$ / %", Style::default().fg(theme.accent)),
            Span::raw(" end / start of line"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+L", Style::default().fg(theme.accent)),
            Span::raw(" clear logs (Job Output)"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+Shift+S", Style::default().fg(theme.accent)),
            Span::raw(" toggle syntax highlight"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+B", Style::default().fg(theme.accent)),
            Span::raw(" toggle debug mode"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+R", Style::default().fg(theme.accent)),
            Span::raw(" reseed visualizer"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+Y", Style::default().fg(theme.accent)),
            Span::raw(" toggle seed source (Editor/Notes)"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+A", Style::default().fg(theme.accent)),
            Span::raw(" apply best rule / variant"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+G", Style::default().fg(theme.accent)),
            Span::raw(" toggle visualizer search"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+O", Style::default().fg(theme.accent)),
            Span::raw(" cycle visualizer auto-stop"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+T", Style::default().fg(theme.accent)),
            Span::raw(" toggle wrap mode"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+N", Style::default().fg(theme.accent)),
            Span::raw(" snapshot visualizer"),
        ]),
        Line::from(vec![
            Span::styled("Space", Style::default().fg(theme.accent)),
            Span::raw(" pause/resume (Visualizer focus)"),
        ]),
        Line::from(vec![
            Span::styled("+ / -", Style::default().fg(theme.accent)),
            Span::raw(" speed up/down (Visualizer focus)"),
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
        .style(Style::default().bg(theme.selection_bg).fg(theme.foreground));

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1)])
        .split(block.inner(area))[0];

    let para =
        Paragraph::new(lines).style(Style::default().bg(theme.selection_bg).fg(theme.foreground));

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}
