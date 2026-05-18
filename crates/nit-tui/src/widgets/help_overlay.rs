//! Keybinding reference popup (F1 / ?). The section list, key labels, and
//! descriptions are load-bearing for tests and user docs — preserve their
//! exact text. This module only handles layout, styling, and scrolling.

use std::sync::OnceLock;

use nit_core::AppState;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;
use nit_core::UiSelectionPane;

type Section = (&'static str, &'static [(&'static str, &'static str)]);

/// Border padding applied by the outer frame — width/height are computed from
/// the screen size minus this amount before clamping.
const FRAME_PADDING: u16 = 4;
const WIDTH_MIN: u16 = 30;
const WIDTH_MAX: u16 = 110;
const HEIGHT_MIN: u16 = 12;
const HEIGHT_MAX: u16 = 36;

/// Compute the preferred popup dimensions from the host `screen`: shrinks
/// for small terminals and clamps to the width/height bounds above.
pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_sub(FRAME_PADDING)).clamp(WIDTH_MIN, WIDTH_MAX);
    let height = (screen.height.saturating_sub(FRAME_PADDING)).clamp(HEIGHT_MIN, HEIGHT_MAX);
    (width, height)
}

/// The overlay content is static — theme only affects styling, not line
/// structure — so we can compute the row count once. Used by the scroll
/// hot path so wheel ticks don't rebuild the full styled buffer just to
/// clamp `max_scroll`.
pub fn line_count() -> usize {
    static CACHED: OnceLock<usize> = OnceLock::new();
    *CACHED.get_or_init(|| build_lines(&Theme::default()).len())
}

fn row(key: &'static str, desc: &'static str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(key, Style::default().fg(theme.accent)),
        Span::raw(desc),
    ])
}

/// Build the full styled line buffer for the help popup. Section headers use
/// the focused-title style, key labels are accent-colored, and descriptions
/// render in the default foreground.
pub fn build_lines(theme: &Theme) -> Vec<Line<'static>> {
    let heading_style = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::BOLD);

    let sections: &[Section] = &[
        (
            "GLOBAL",
            &[
                ("Ctrl+Q", " quit (confirm if dirty)"),
                ("Ctrl+S", " save"),
                ("Ctrl+T", " toggle NITTree (file tree)"),
                ("Ctrl+P", " fuzzy file search"),
                ("Ctrl+F", " content search"),
                ("F1 / ?", " toggle help"),
                ("Tab/Shift+Tab", " focus panes"),
                ("Ctrl+1/2/3", " focus Editor / Agent Ops / Agent Chat"),
                (
                    "Ctrl+H/J/K/L",
                    " focus panes (left/down/up/right; not in Visualizer)",
                ),
                ("Ctrl+B", " toggle debug mode (non-Visualizer focus)"),
                ("Ctrl+Enter", " run Petri Dish (active app)"),
                ("Ctrl+^ / Ctrl+6", " show hidden Petri Dish"),
                (":", " command prompt (Normal mode)"),
            ],
        ),
        (
            "NITTREE (EDITOR OVERLAY)",
            &[
                ("Esc / q", " close tree"),
                ("j/k / Up/Down", " move selection"),
                ("PageUp/PageDown", " jump by page"),
                ("Home/End", " jump to top/bottom"),
                ("Enter", " open file (closes tree)"),
                ("r", " refresh tree"),
                (".", " toggle hidden files"),
                ("i", " toggle ignored files"),
            ],
        ),
        (
            "COMMANDS (:)",
            &[
                (":q", " quit (confirm if dirty)"),
                (":w / :write", " save current file"),
                (
                    ":wq / :x",
                    " save + quit (file launch) or save + switch to last buffer / NITTree (dir launch)",
                ),
                (":e / :edit <path>", " open file at path (workspace-relative)"),
                (":help / :commands", " open this help overlay"),
                (":run", " run active app"),
                (":tree / :nittree / :explore", " toggle NITTree"),
                (":find / :ff", " open fuzzy file search"),
                (":grep / :rg / :search", " open content search"),
                (
                    ":gol run|hide|show|stop",
                    " GoL Petri Dish controls (alias: :life ...)",
                ),
                (
                    ":petri hide|show",
                    " GoL Petri Dish visibility (alias for :gol hide|show)",
                ),
                (":gol rule [id|B/S]", " set rule / show current"),
                (":gol rules", " list GoL rules"),
                (
                    ":gol seed | :gol encoder",
                    " cycle seed view/encoder (aliases: :seed view|encoder)",
                ),
                (":games run|hide|show|stop|status|export", " games controls"),
                (
                    ":games run [force] <fsm|ca|tm> {…} [tm_steps]",
                    " run family tournament; use force to bypass speed caps",
                ),
                (":games runs", " browse saved runs"),
                (
                    ":games replay",
                    " open replay selector (requires loaded run)",
                ),
                (
                    ":games history | :history",
                    " open match history plot popup",
                ),
                (
                    ":games strategy [run]",
                    " open strategy inspector for loaded run",
                ),
                (
                    ":games strategies all|config",
                    " open strategy inspector from config",
                ),
                (":games inspect <id>", " introspect a strategy by id"),
                (
                    ":games inspect <fsm_index>",
                    " inspect FSM notebook index (defaults to {index,2,2})",
                ),
                (
                    ":games inspect <id> {rule,states,symbols}",
                    " inspect TM tuple; use id=fsm for FSM {index,states,k}",
                ),
                (
                    ":games inspect {rule,states,symbols}",
                    " inspect a one-sided TM rule tuple",
                ),
                (
                    ":games tm [run|config] <input> [steps] [id]",
                    " simulate one-sided TM on integer input",
                ),
                (
                    ":games tm {rule,states,symbols} <input> [steps]",
                    " simulate a rule-code TM without config",
                ),
                (
                    ":games ca [run|config] <input> [steps] [id]",
                    " simulate shrinking CA on integer input",
                ),
                (
                    ":games ca {n,k,r} <input> [steps]",
                    " simulate a CA rule tuple (t defaults to 10)",
                ),
                (
                    ":games analyze[se] [path] [tail=N] [samples=N]",
                    " analyze last/specified history log",
                ),
            ],
        ),
        (
            "GAMES PETRI DISH (POPUP)",
            &[
                ("Esc", " close tournament"),
                ("Space", " pause / resume"),
                ("Enter", " step (when paused)"),
                ("+ / -", " speed up / down"),
                ("Tab", " toggle tournament / inspector"),
                ("← / →", " adjust inspector window"),
                ("H", " hide (continues running)"),
            ],
        ),
        (
            "EDITOR (FOCUSED)",
            &[
                ("Ctrl/Cmd+A", " select all (also Scratchpad)"),
                ("Ctrl/Cmd+C", " copy selection (also Scratchpad)"),
                ("Ctrl/Cmd+X", " cut selection (also Scratchpad)"),
                ("Ctrl/Cmd+V", " paste (replaces selection; also Scratchpad)"),
                ("Ctrl/Alt+←/→", " move by word (also Scratchpad)"),
                (
                    "Ctrl/Alt+Backspace/Delete",
                    " delete word (also Scratchpad)",
                ),
                ("Enter", " newline (preserves indentation)"),
                ("Esc", " switch to Normal mode"),
                ("H/J/K/L", " move in Normal mode"),
                ("I", " enter Insert mode"),
                ("a", " append + Insert (Normal mode)"),
                ("v", " Visual mode (Normal mode)"),
                ("o", " open line below + Insert (Normal mode)"),
                ("Shift+O", " open line above + Insert (Normal mode)"),
                ("JJ", " save + Normal (Insert mode)"),
                ("Shift+S", " toggle syntax highlight (Editor focus)"),
                ("GG / Shift+G", " top / bottom"),
                ("u / Ctrl/Cmd+Z", " undo"),
                ("Shift+R / Ctrl+Y / Ctrl/Cmd+Shift+Z", " redo"),
                ("e / b", " word end / word back (Normal mode)"),
                ("y", " yank selection (Visual mode)"),
                ("yy", " yank line (Normal mode)"),
                ("d", " delete selection (Visual mode)"),
                ("p", " paste (Normal mode)"),
                ("Shift+P", " paste above (Normal mode)"),
                ("dd", " delete line (Normal mode)"),
                ("$ / %", " end / start of line"),
            ],
        ),
        (
            "AGENT OPS (FOCUSED)",
            &[
                ("Tab / Shift+Tab / ←/→", " cycle Ops tabs"),
                ("j/k or ↑/↓", " move selection"),
                ("Enter", " focus Agent Chat with current context"),
                ("n", " create new mission (mock runner)"),
                ("r / s / x", " MCP reconnect / start / stop (MCP tab)"),
            ],
        ),
        (
            "AGENT CHAT (FOCUSED)",
            &[
                ("Enter", " send message (@all <msg> broadcasts)"),
                ("Shift+Enter", " newline (preserves indentation)"),
                ("Tab", " insert tab"),
                ("Ctrl/Cmd+A", " select all"),
                ("Cmd+C / Ctrl+Shift+C", " copy selection"),
                ("Cmd+X / Ctrl+Shift+X", " cut selection"),
                ("Ctrl/Cmd+V", " paste"),
                ("Ctrl+U", " clear input"),
                ("Ctrl+C", " clear input (copies if selection)"),
                ("Esc", " clear selection / thread selection"),
                (
                    "←/→ / Home/End",
                    " move cursor (Shift selects; Ctrl/Alt moves by word)",
                ),
                ("↑/↓", " move cursor / prompt history"),
                ("Ctrl+↑/↓", " scroll chat thread"),
            ],
        ),
        (
            "VISUALIZER (FOCUSED)",
            &[
                ("Ctrl+E", " cycle encoder"),
                ("Ctrl+V", " toggle view (GENOME ↔ PLATE)"),
                ("Ctrl+R", " cycle seed view (genome/plate/map/stats)"),
                (
                    "Ctrl+M",
                    " cycle plate render (solid/half/braille/tissue/heat)",
                ),
                ("Ctrl+Y", " toggle seed source"),
                ("Ctrl+A", " apply seed proposal"),
                ("Ctrl+G", " toggle seed search"),
                ("Ctrl+N", " snapshot seed"),
                ("Ctrl+Shift+V", " cycle seed overlays"),
                ("Arrows / HJKL", " move genome inspector (Visualizer focus)"),
                ("Home / End", " inspector jump to edges"),
                ("0 / $", " inspector jump to edges (fallback)"),
                ("G + digits + Enter", " jump to genome index"),
                ("C", " center inspector"),
                ("I", " toggle inspector"),
            ],
        ),
        (
            "PETRI DISH (POPUP)",
            &[
                ("Esc", " close popup"),
                ("Space", " pause/resume"),
                ("Enter", " step one generation"),
                ("+ / -", " speed up/down"),
                ("S", " snapshot sim"),
                ("Ctrl+R", " reseed from current code"),
                ("T", " toggle wrap mode"),
                ("O", " cycle auto-stop"),
                ("G", " toggle rule search"),
                ("A", " apply best rule"),
                ("H", " hide popup (sim keeps running)"),
            ],
        ),
    ];

    let total: usize = sections.iter().map(|(_, rows)| rows.len() + 1).sum();
    let mut lines = Vec::with_capacity(total);
    for (title, rows) in sections {
        lines.push(Line::from(Span::styled(*title, heading_style)));
        lines.extend(rows.iter().map(|(key, desc)| row(key, desc, theme)));
    }
    lines
}

/// Render the help overlay inside `area`. Scroll offset is clamped against
/// the total line count; only the viewport window is walked through the
/// selection pipeline so mouse-selection stays O(viewport_rows).
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let panel_style = Style::default().bg(theme.selection_bg).fg(theme.foreground);
    let outer_frame = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            "HELP",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .style(panel_style);
    let content_area = outer_frame.inner(area);

    let all_lines = build_lines(theme);
    let viewport_rows = content_area.height as usize;
    let max_offset = all_lines.len().saturating_sub(viewport_rows);
    let offset = state.help_scroll.min(max_offset);
    let window: Vec<_> = all_lines
        .into_iter()
        .skip(offset)
        .take(viewport_rows)
        .collect();
    let with_selection = apply_ui_selection(
        window,
        state.ui_selection.as_ref(),
        UiSelectionPane::HelpPopup,
        theme.cursor_line_bg,
        offset,
    );

    frame.render_widget(Clear, area);
    frame.render_widget(outer_frame, area);
    frame.render_widget(
        Paragraph::new(with_selection).style(panel_style),
        content_area,
    );
}
