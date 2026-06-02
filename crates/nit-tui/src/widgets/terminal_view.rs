//! Shared vt100-grid → ratatui render for the T6 terminal pane and the T7
//! terminal popup. `render_screen` paints the grid only; the caller places the
//! hardware cursor via `cursor_position` so stacked panes/popups never fight
//! over the single frame cursor slot.

use nit_core::{UiSelection, UiSelectionPane};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::pty::PtySession;
use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

pub fn render_screen(
    frame: &mut Frame,
    area: Rect,
    session: &PtySession,
    theme: &Theme,
    selection: Option<&UiSelection>,
    pane: UiSelectionPane,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let parser = session.screen();
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let visible_rows = area.height.min(rows);
    let visible_cols = area.width.min(cols);

    // `cell.contents()` borrows the parser, but `apply_ui_selection` needs
    // `'static` lines, so each glyph is cloned into an owned span.
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible_rows as usize);
    for row in 0..visible_rows {
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(visible_cols as usize);
        for col in 0..visible_cols {
            match screen.cell(row, col) {
                Some(cell) if cell.has_contents() => {
                    spans.push(Span::styled(
                        cell.contents().to_string(),
                        cell_style(cell, theme),
                    ));
                }
                Some(cell) => spans.push(Span::styled(" ".to_string(), cell_style(cell, theme))),
                None => spans.push(Span::raw(" ".to_string())),
            }
        }
        lines.push(Line::from(spans));
    }
    // The embedded PTY has no scrollback, so the visible grid starts at line 0
    // and selection rows map 1:1 to screen rows (scroll = 0).
    let lines = apply_ui_selection(lines, selection, pane, theme.selection_bg, 0);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.background)),
        area,
    );
}

/// The terminal's visible grid as plain strings, one per row, each padded to
/// the rendered column width so character indices line up with what
/// `render_screen` paints. Mouse handlers snapshot this (with the inner `area`)
/// into `AppState::terminal_select_regions` so drag-select + copy can resolve
/// a click to a row/column and slice the selected text — all without reaching
/// the `PtySession` from the input layer.
pub fn visible_text_lines(area: Rect, session: &PtySession) -> Vec<String> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    let parser = session.screen();
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let visible_rows = area.height.min(rows);
    let visible_cols = area.width.min(cols);

    let mut lines = Vec::with_capacity(visible_rows as usize);
    for row in 0..visible_rows {
        let mut text = String::with_capacity(visible_cols as usize);
        for col in 0..visible_cols {
            match screen.cell(row, col) {
                Some(cell) if cell.has_contents() => text.push_str(cell.contents()),
                _ => text.push(' '),
            }
        }
        lines.push(text);
    }
    lines
}

/// Screen-space cursor cell, or `None` when the shell hid it or it sits
/// outside the rendered window.
pub fn cursor_position(area: Rect, session: &PtySession) -> Option<(u16, u16)> {
    let parser = session.screen();
    let screen = parser.screen();
    if screen.hide_cursor() {
        return None;
    }
    let (row, col) = screen.cursor_position();
    if row >= area.height || col >= area.width {
        return None;
    }
    Some((area.x + col, area.y + row))
}

fn cell_style(cell: &vt100::Cell, theme: &Theme) -> Style {
    let mut style = Style::default()
        .fg(vt_color(cell.fgcolor(), theme.foreground))
        .bg(vt_color(cell.bgcolor(), theme.background));
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

fn vt_color(color: vt100::Color, fallback: Color) -> Color {
    match color {
        vt100::Color::Default => fallback,
        vt100::Color::Idx(idx) => Color::Indexed(idx),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
