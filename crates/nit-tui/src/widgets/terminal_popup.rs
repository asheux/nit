//! Modal terminal popup overlay (`Ctrl+Shift+T`). A centered, dimmed-behind
//! box hosting the shared `PtySession` shell. Distinct from the T6 pane: it
//! overlays the layout without disturbing it and hides (not kills) on close,
//! so re-opening resumes the same session.

use std::path::Path;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear},
    Frame,
};

use crate::pty::{PtySession, PtySize};
use crate::theme::Theme;
use crate::widgets::terminal_view;

const WIDTH_PCT: u16 = 55;
const HEIGHT_PCT: u16 = 70;

/// Centered overlay rect at 55% width / 70% height of `screen`, floored so a
/// cramped terminal still yields a bordered box rather than a sliver.
pub fn popup_rect(screen: Rect) -> Rect {
    let width = pct(screen.width, WIDTH_PCT).max(20).min(screen.width);
    let height = pct(screen.height, HEIGHT_PCT).max(6).min(screen.height);
    let x = screen.x + screen.width.saturating_sub(width) / 2;
    let y = screen.y + screen.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn pct(value: u16, percent: u16) -> u16 {
    (value as u32 * percent as u32 / 100) as u16
}

/// Dim every cell behind the popup so the modal reads as focused. Patches the
/// DIM modifier in without disturbing fg/bg; the popup's own `Clear` then
/// repaints its own area crisply on top.
fn dim_behind(frame: &mut Frame, area: Rect) {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buffer.get_mut(x, y).set_style(dim);
        }
    }
}

/// Render the popup over `screen` and return the absolute hardware-cursor cell
/// (or `None` when the shell hid it). The PTY is resized to the inner area each
/// frame so the shell's winsize tracks the viewport. `cwd` is the cwd to label
/// the title with — the caller polls the shell's live cwd via `ShellCwdProbe`
/// so `cd` inside the popup updates the header, falling back to the cwd pinned
/// at spawn time when sysinfo can't read it.
pub fn render(
    frame: &mut Frame,
    screen: Rect,
    session: &PtySession,
    cwd: Option<&Path>,
    theme: &Theme,
) -> Option<(u16, u16)> {
    dim_behind(frame, screen);
    let area = popup_rect(screen);
    let title_text = match cwd {
        Some(path) => format!(" terminal — {} ", path.display()),
        None => " terminal ".to_string(),
    };
    let title_style = Style::default()
        .fg(theme.title_focused)
        .add_modifier(Modifier::BOLD);
    let hint_style = Style::default().fg(theme.border);
    let title_line = Line::from(vec![
        Span::styled(title_text, title_style),
        Span::raw("  "),
        // Both popup-close affordances on one line so operators don't
        // have to remember a chord. Esc Esc reaches the shell on the
        // first press (so vim / less can react) and only closes on the
        // double-tap; Ctrl+Shift+T closes immediately.
        Span::styled("[ Esc Esc · Ctrl+Shift+T close ]", hint_style),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(title_line)
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let _ = session.resize(PtySize {
        rows: inner.height,
        cols: inner.width,
    });
    terminal_view::render_screen(frame, inner, session, theme);
    terminal_view::cursor_position(inner, session)
}

#[cfg(test)]
#[path = "../tests/terminal_popup.rs"]
mod tests;
