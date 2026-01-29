use nit_core::AppState;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;
use nit_core::UiSelectionPane;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_sub(4)).min(80).max(30);
    let height = (screen.height.saturating_sub(4)).min(36).max(12);
    (width, height)
}

pub fn build_lines(theme: &Theme) -> Vec<Line<'static>> {
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
        Span::raw(" run Petri Dish (active app)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+^", Style::default().fg(theme.accent)),
        Span::raw(" show hidden Petri Dish"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":", Style::default().fg(theme.accent)),
        Span::raw(" command prompt (Normal mode)"),
    ]));
    lines.push(Line::from(vec![Span::styled(
        "COMMANDS (:)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled(":run", Style::default().fg(theme.accent)),
        Span::raw(" run active app"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":gol run|hide|show|stop", Style::default().fg(theme.accent)),
        Span::raw(" GoL petri controls"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":gol rule [id|B/S]", Style::default().fg(theme.accent)),
        Span::raw(" set rule / show current"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":gol rules", Style::default().fg(theme.accent)),
        Span::raw(" list GoL rules"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":gol seed | :gol encoder",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" cycle seed view/encoder"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games run|hide|show|status|export",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" games controls"),
    ]));
    lines.push(Line::from(vec![Span::styled(
        "GAMES PETRI DISH (POPUP)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Esc", Style::default().fg(theme.accent)),
        Span::raw(" close tournament"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Space", Style::default().fg(theme.accent)),
        Span::raw(" pause / resume"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.accent)),
        Span::raw(" step (when paused)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("+ / -", Style::default().fg(theme.accent)),
        Span::raw(" speed up / down"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Tab", Style::default().fg(theme.accent)),
        Span::raw(" toggle tournament / inspector"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("← / →", Style::default().fg(theme.accent)),
        Span::raw(" adjust inspector window"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("H", Style::default().fg(theme.accent)),
        Span::raw(" hide (continues running)"),
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
        Span::raw(" cycle encoder"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+V", Style::default().fg(theme.accent)),
        Span::raw(" toggle view (GENOME ↔ PLATE)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+R", Style::default().fg(theme.accent)),
        Span::raw(" cycle seed view (genome/plate/map/stats)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+M", Style::default().fg(theme.accent)),
        Span::raw(" cycle plate render (solid/half/braille/tissue/heat)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+Y", Style::default().fg(theme.accent)),
        Span::raw(" toggle seed source (Editor/Notes)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+A", Style::default().fg(theme.accent)),
        Span::raw(" apply seed proposal"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+G", Style::default().fg(theme.accent)),
        Span::raw(" toggle seed search"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+N", Style::default().fg(theme.accent)),
        Span::raw(" snapshot seed"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+Shift+V", Style::default().fg(theme.accent)),
        Span::raw(" cycle seed overlays"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Arrows / HJKL", Style::default().fg(theme.accent)),
        Span::raw(" move genome inspector (Visualizer focus)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Home / End", Style::default().fg(theme.accent)),
        Span::raw(" inspector jump to edges"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("0 / $", Style::default().fg(theme.accent)),
        Span::raw(" inspector jump to edges (fallback)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("G + digits + Enter", Style::default().fg(theme.accent)),
        Span::raw(" jump to genome index"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("C", Style::default().fg(theme.accent)),
        Span::raw(" center inspector"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("I", Style::default().fg(theme.accent)),
        Span::raw(" toggle inspector"),
    ]));

    lines.push(Line::from(vec![Span::styled(
        "PETRI DISH (POPUP)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Esc", Style::default().fg(theme.accent)),
        Span::raw(" close popup"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Space", Style::default().fg(theme.accent)),
        Span::raw(" pause/resume"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.accent)),
        Span::raw(" step one generation"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("+ / -", Style::default().fg(theme.accent)),
        Span::raw(" speed up/down"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("S", Style::default().fg(theme.accent)),
        Span::raw(" snapshot sim"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+R", Style::default().fg(theme.accent)),
        Span::raw(" reseed from current code"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("T", Style::default().fg(theme.accent)),
        Span::raw(" toggle wrap mode"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("O", Style::default().fg(theme.accent)),
        Span::raw(" cycle auto-stop"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("G", Style::default().fg(theme.accent)),
        Span::raw(" toggle rule search"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("A", Style::default().fg(theme.accent)),
        Span::raw(" apply best rule"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("H", Style::default().fg(theme.accent)),
        Span::raw(" hide popup (sim keeps running)"),
    ]));

    lines.push(Line::from(vec![Span::styled(
        "COMMAND PROMPT",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled(":gol hide", Style::default().fg(theme.accent)),
        Span::raw(" hide Petri Dish (keep running)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":gol show", Style::default().fg(theme.accent)),
        Span::raw(" show Petri Dish"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games analyze", Style::default().fg(theme.accent)),
        Span::raw(" analyze last Games history log"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games analyze <path>", Style::default().fg(theme.accent)),
        Span::raw(" analyze specific history log"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games runs", Style::default().fg(theme.accent)),
        Span::raw(" browse saved runs"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games replay", Style::default().fg(theme.accent)),
        Span::raw(" inspect a match replay"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games strategy", Style::default().fg(theme.accent)),
        Span::raw(" inspect a strategy definition"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games strategies all", Style::default().fg(theme.accent)),
        Span::raw(" list all strategies from config"),
    ]));
    lines
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let lines = build_lines(theme);

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

    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = state.help_scroll.min(max_scroll);
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
    let visible = apply_ui_selection(
        visible,
        state.ui_selection.as_ref(),
        UiSelectionPane::HelpPopup,
        theme.cursor_line_bg,
        scroll,
    );
    let para = Paragraph::new(visible)
        .style(Style::default().bg(theme.selection_bg).fg(theme.foreground));

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}
