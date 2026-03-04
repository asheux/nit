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
    let width = (screen.width.saturating_sub(4)).clamp(30, 110);
    let height = (screen.height.saturating_sub(4)).clamp(12, 36);
    (width, height)
}

pub fn build_lines(theme: &Theme) -> Vec<Line<'static>> {
    let heading_style = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::BOLD);
    let mut lines = Vec::with_capacity(256);

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
        Span::styled("Ctrl+T", Style::default().fg(theme.accent)),
        Span::raw(" toggle NITTree (file tree)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+P", Style::default().fg(theme.accent)),
        Span::raw(" fuzzy file search"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+F", Style::default().fg(theme.accent)),
        Span::raw(" content search"),
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
        Span::styled("Ctrl+1/2/3", Style::default().fg(theme.accent)),
        Span::raw(" focus Editor / Agent Ops / Agent Chat"),
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
        Span::styled("Ctrl+^ / Ctrl+6", Style::default().fg(theme.accent)),
        Span::raw(" show hidden Petri Dish"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":", Style::default().fg(theme.accent)),
        Span::raw(" command prompt (Normal mode)"),
    ]));

    lines.push(Line::from(vec![Span::styled(
        "NITTREE (EDITOR OVERLAY)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Esc / q", Style::default().fg(theme.accent)),
        Span::raw(" close tree"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("j/k / Up/Down", Style::default().fg(theme.accent)),
        Span::raw(" move selection"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("PageUp/PageDown", Style::default().fg(theme.accent)),
        Span::raw(" jump by page"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Home/End", Style::default().fg(theme.accent)),
        Span::raw(" jump to top/bottom"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.accent)),
        Span::raw(" open file (closes tree)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("r", Style::default().fg(theme.accent)),
        Span::raw(" refresh tree"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(".", Style::default().fg(theme.accent)),
        Span::raw(" toggle hidden files"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("i", Style::default().fg(theme.accent)),
        Span::raw(" toggle ignored files"),
    ]));
    lines.push(Line::from(vec![Span::styled(
        "COMMANDS (:)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled(":q", Style::default().fg(theme.accent)),
        Span::raw(" quit (confirm if dirty)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":help / :commands", Style::default().fg(theme.accent)),
        Span::raw(" open this help overlay"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":run", Style::default().fg(theme.accent)),
        Span::raw(" run active app"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":tree / :nittree / :explore",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" toggle NITTree"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":find / :ff", Style::default().fg(theme.accent)),
        Span::raw(" open fuzzy file search"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":grep / :rg / :search", Style::default().fg(theme.accent)),
        Span::raw(" open content search"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":gol run|hide|show|stop", Style::default().fg(theme.accent)),
        Span::raw(" GoL Petri Dish controls (alias: :life ...)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":petri hide|show", Style::default().fg(theme.accent)),
        Span::raw(" GoL Petri Dish visibility (alias for :gol hide|show)"),
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
        Span::raw(" cycle seed view/encoder (aliases: :seed view|encoder)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games run|hide|show|stop|status|export",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" games controls"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games run [force] <fsm|ca|tm> {…}",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" run family tournament; use force to bypass speed caps"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games runs", Style::default().fg(theme.accent)),
        Span::raw(" browse saved runs"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games replay", Style::default().fg(theme.accent)),
        Span::raw(" open replay selector (requires loaded run)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games history | :history",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" open match history plot popup"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games strategy [run]", Style::default().fg(theme.accent)),
        Span::raw(" open strategy inspector for loaded run"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games strategies all|config",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" open strategy inspector from config"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(":games inspect <id>", Style::default().fg(theme.accent)),
        Span::raw(" introspect a strategy by id"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games inspect <fsm_index>",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" inspect FSM notebook index (defaults to {index,2,2})"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games inspect <id> {rule,states,symbols}",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" inspect TM tuple; use id=fsm for FSM {index,states,k}"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games inspect {rule,states,symbols}",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" inspect a one-sided TM rule tuple"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games tm [run|config] <input> [steps] [id]",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" simulate one-sided TM on integer input"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games tm {rule,states,symbols} <input> [steps]",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" simulate a rule-code TM without config"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games ca [run|config] <input> [steps] [id]",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" simulate shrinking CA on integer input"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games ca {n,k,r} <input> [steps]",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" simulate a CA rule tuple (t defaults to 10)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            ":games analyze[se] [path] [tail=N] [samples=N]",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" analyze last/specified history log"),
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
        "EDITOR (FOCUSED)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl/Cmd+A", Style::default().fg(theme.accent)),
        Span::raw(" select all (also Scratchpad)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl/Cmd+C", Style::default().fg(theme.accent)),
        Span::raw(" copy selection (also Scratchpad)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl/Cmd+X", Style::default().fg(theme.accent)),
        Span::raw(" cut selection (also Scratchpad)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl/Cmd+V", Style::default().fg(theme.accent)),
        Span::raw(" paste (replaces selection; also Scratchpad)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl/Alt+←/→", Style::default().fg(theme.accent)),
        Span::raw(" move by word (also Scratchpad)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            "Ctrl/Alt+Backspace/Delete",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" delete word (also Scratchpad)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.accent)),
        Span::raw(" newline (preserves indentation)"),
    ]));
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
        Span::styled("u / Ctrl/Cmd+Z", Style::default().fg(theme.accent)),
        Span::raw(" undo"),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            "Shift+R / Ctrl+Y / Ctrl/Cmd+Shift+Z",
            Style::default().fg(theme.accent),
        ),
        Span::raw(" redo"),
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
        "AGENT OPS (FOCUSED)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Tab / Shift+Tab / ←/→", Style::default().fg(theme.accent)),
        Span::raw(" cycle Ops tabs"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("j/k or ↑/↓", Style::default().fg(theme.accent)),
        Span::raw(" move selection"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.accent)),
        Span::raw(" focus Agent Chat with current context"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("n", Style::default().fg(theme.accent)),
        Span::raw(" create new mission (mock runner)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("r / s / x", Style::default().fg(theme.accent)),
        Span::raw(" MCP reconnect / start / stop (MCP tab)"),
    ]));

    lines.push(Line::from(vec![Span::styled(
        "AGENT CHAT (FOCUSED)",
        heading_style,
    )]));
    lines.push(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.accent)),
        Span::raw(" send message (@all <msg> broadcasts)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Shift+Enter", Style::default().fg(theme.accent)),
        Span::raw(" newline (preserves indentation)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Tab", Style::default().fg(theme.accent)),
        Span::raw(" insert tab"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl/Cmd+A", Style::default().fg(theme.accent)),
        Span::raw(" select all"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Cmd+C / Ctrl+Shift+C", Style::default().fg(theme.accent)),
        Span::raw(" copy selection"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Cmd+X / Ctrl+Shift+X", Style::default().fg(theme.accent)),
        Span::raw(" cut selection"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl/Cmd+V", Style::default().fg(theme.accent)),
        Span::raw(" paste"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+U", Style::default().fg(theme.accent)),
        Span::raw(" clear input"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+C", Style::default().fg(theme.accent)),
        Span::raw(" clear input (copies if selection)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Esc", Style::default().fg(theme.accent)),
        Span::raw(" clear selection / thread selection"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("←/→ / Home/End", Style::default().fg(theme.accent)),
        Span::raw(" move cursor (Shift selects; Ctrl/Alt moves by word)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("↑/↓", Style::default().fg(theme.accent)),
        Span::raw(" move cursor / prompt history"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Ctrl+↑/↓", Style::default().fg(theme.accent)),
        Span::raw(" scroll chat thread"),
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
        Span::raw(" toggle seed source"),
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
    let para =
        Paragraph::new(visible).style(Style::default().bg(theme.selection_bg).fg(theme.foreground));

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}
