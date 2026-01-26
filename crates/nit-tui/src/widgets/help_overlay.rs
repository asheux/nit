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
    let heading_style = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();

    lines.push(Line::from(vec![Span::styled("GLOBAL", heading_style)]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+Q", Style::default().fg(theme.accent)),
        Span::raw(" quit (confirm if dirty)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+S", Style::default().fg(theme.accent)),
        Span::raw(" save"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("F1 / ?", Style::default().fg(theme.accent)),
        Span::raw(" toggle help"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Tab/Shift+Tab", Style::default().fg(theme.accent)),
        Span::raw(" focus panes"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+H/J/K/L", Style::default().fg(theme.accent)),
        Span::raw(" focus panes (left/down/up/right; not in Visualizer)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+B", Style::default().fg(theme.accent)),
        Span::raw(" toggle debug mode (non-Visualizer focus)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+Enter", Style::default().fg(theme.accent)),
        Span::raw(" run visualizer (any focus)"),
    ]));

    lines.push(Line::from(vec![Span::styled(
        "EDITOR / NOTES (FOCUSED)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Esc", Style::default().fg(theme.accent)),
        Span::raw(" switch to Normal mode"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("H/J/K/L", Style::default().fg(theme.accent)),
        Span::raw(" move in Normal mode"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("I", Style::default().fg(theme.accent)),
        Span::raw(" enter Insert mode"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("a", Style::default().fg(theme.accent)),
        Span::raw(" append + Insert (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("v", Style::default().fg(theme.accent)),
        Span::raw(" Visual mode (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("o", Style::default().fg(theme.accent)),
        Span::raw(" open line below + Insert (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Shift+O", Style::default().fg(theme.accent)),
        Span::raw(" open line above + Insert (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("JJ", Style::default().fg(theme.accent)),
        Span::raw(" save + Normal (Insert mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Shift+S", Style::default().fg(theme.accent)),
        Span::raw(" toggle syntax highlight (Editor focus)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("GG / Shift+G", Style::default().fg(theme.accent)),
        Span::raw(" top / bottom"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("u", Style::default().fg(theme.accent)),
        Span::raw(" undo (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Shift+R", Style::default().fg(theme.accent)),
        Span::raw(" redo (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("e / b", Style::default().fg(theme.accent)),
        Span::raw(" word end / word back (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("y", Style::default().fg(theme.accent)),
        Span::raw(" yank selection (Visual mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("yy", Style::default().fg(theme.accent)),
        Span::raw(" yank line (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("d", Style::default().fg(theme.accent)),
        Span::raw(" delete selection (Visual mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("p", Style::default().fg(theme.accent)),
        Span::raw(" paste (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Shift+P", Style::default().fg(theme.accent)),
        Span::raw(" paste above (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("dd", Style::default().fg(theme.accent)),
        Span::raw(" delete line (Normal mode)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("$ / %", Style::default().fg(theme.accent)),
        Span::raw(" end / start of line"),
    ]));

    lines.push(Line::from(vec![Span::styled(
        "JOB OUTPUT (FOCUSED)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+L", Style::default().fg(theme.accent)),
        Span::raw(" clear logs"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+Space", Style::default().fg(theme.accent)),
        Span::raw(" pause/resume job updates"),
    ]));

    lines.push(Line::from(vec![Span::styled(
        "VISUALIZER (FOCUSED)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+E", Style::default().fg(theme.accent)),
        Span::raw(" return to ASCII view"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+R", Style::default().fg(theme.accent)),
        Span::raw(" reseed visualizer"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+Y", Style::default().fg(theme.accent)),
        Span::raw(" toggle seed source (Editor/Notes)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+A", Style::default().fg(theme.accent)),
        Span::raw(" apply best rule / variant"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+G", Style::default().fg(theme.accent)),
        Span::raw(" toggle visualizer search"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+O", Style::default().fg(theme.accent)),
        Span::raw(" cycle visualizer auto-stop"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+T", Style::default().fg(theme.accent)),
        Span::raw(" toggle wrap mode"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+N", Style::default().fg(theme.accent)),
        Span::raw(" snapshot visualizer"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+M", Style::default().fg(theme.accent)),
        Span::raw(" cycle render mode"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+J", Style::default().fg(theme.accent)),
        Span::raw(" toggle age shading"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+K", Style::default().fg(theme.accent)),
        Span::raw(" toggle trails"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+B", Style::default().fg(theme.accent)),
        Span::raw(" toggle bbox overlay"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+H", Style::default().fg(theme.accent)),
        Span::raw(" toggle heat overlay"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+L", Style::default().fg(theme.accent)),
        Span::raw(" toggle scanlines"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Space", Style::default().fg(theme.accent)),
        Span::raw(" pause/resume"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("+ / -", Style::default().fg(theme.accent)),
        Span::raw(" speed up/down"),
    ]));

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
