use crossterm::event::{KeyCode, KeyEvent};
use nit_core::{skills, AppKind, AppState, GateMonitorSubView, PaneId};
use ratatui::{
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders},
};

use crate::{theme::Theme, widgets::picker_utils::wrap_text};

fn active(state: &AppState) -> bool {
    state.app_kind == AppKind::Gol && state.gate_monitor_sub_view == GateMonitorSubView::Skills
}

pub(crate) fn focused(state: &AppState) -> bool {
    active(state) && state.focus == PaneId::GateMonitor
}

pub(super) fn lines(state: &AppState, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let hint = Style::default().fg(theme.accent);
    if state.skill_reader_open {
        let mut out = vec![Line::styled("‹ Back  ·  Esc", hint)];
        if let Some(body) = skills::entries()
            .nth(state.selected_skill)
            .and_then(|(name, _)| skills::read(name))
        {
            for line in body.lines() {
                let heading = line.starts_with('#');
                let line = if heading {
                    line.trim_start_matches('#').trim()
                } else {
                    line
                };
                let style = if heading {
                    hint.add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                // Wrap tables as text so every cell remains accessible on a narrow console.
                if line.starts_with('|') && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
                {
                    continue;
                }
                out.extend(
                    wrap_text(line, width)
                        .into_iter()
                        .map(|row| Line::styled(row, style)),
                );
            }
        }
        return out;
    }

    let mut out = vec![Line::styled("↑↓ choose · Enter read", hint)];
    for (i, (name, _)) in skills::entries().enumerate() {
        if i == state.selected_skill {
            out.push(Line::styled(
                format!("› {name}"),
                Style::default()
                    .fg(theme.foreground)
                    .bg(theme.selection_bg)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            out.push(Line::from(format!("  {name}")));
        }
    }
    if let Some((_, summary)) = skills::entries().nth(state.selected_skill) {
        out.push(Line::from(""));
        out.extend(wrap_text(summary, width).into_iter().map(Line::from));
    }
    out
}

fn switch_reader(state: &mut AppState, open: bool, height: usize) {
    state.skill_reader_open = open;
    state.ui_selection = None;
    state.gate_monitor_scroll = if open {
        0
    } else {
        (state.selected_skill + 2).saturating_sub(height)
    };
    state.gate_monitor_last_max_scroll = usize::MAX;
}

pub(crate) fn handle_key(state: &mut AppState, key: KeyEvent, area: Rect) -> bool {
    if !focused(state) || !key.modifiers.is_empty() {
        return false;
    }
    let height = area.height.saturating_sub(2).max(1) as usize;
    let last = skills::entries().len().saturating_sub(1);
    if state.skill_reader_open {
        let scroll = state.gate_monitor_scroll;
        match key.code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Left => {
                switch_reader(state, false, height)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.gate_monitor_scroll = scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.gate_monitor_scroll = scroll
                    .saturating_add(1)
                    .min(state.gate_monitor_last_max_scroll)
            }
            KeyCode::PageDown => {
                state.gate_monitor_scroll = scroll
                    .saturating_add(height)
                    .min(state.gate_monitor_last_max_scroll)
            }
            KeyCode::PageUp => state.gate_monitor_scroll = scroll.saturating_sub(height),
            KeyCode::Home => state.gate_monitor_scroll = 0,
            KeyCode::End => state.gate_monitor_scroll = state.gate_monitor_last_max_scroll,
            _ => return false,
        }
    } else {
        match key.code {
            KeyCode::Enter => switch_reader(state, true, height),
            KeyCode::Up
            | KeyCode::Char('k')
            | KeyCode::Down
            | KeyCode::Char('j')
            | KeyCode::Home
            | KeyCode::End => {
                state.selected_skill = match key.code {
                    KeyCode::Home => 0,
                    KeyCode::End => last,
                    KeyCode::Up | KeyCode::Char('k') => state.selected_skill.saturating_sub(1),
                    _ => (state.selected_skill + 1).min(last),
                };
                let row = state.selected_skill + 1;
                state.gate_monitor_scroll = state
                    .gate_monitor_scroll
                    .clamp((row + 1).saturating_sub(height), row);
                state.ui_selection = None;
            }
            _ => return false,
        }
    }
    true
}

pub(crate) fn click(state: &mut AppState, area: Rect, point: Position) -> bool {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if !active(state) || !inner.contains(point) {
        return false;
    }
    let row = state
        .gate_monitor_scroll
        .saturating_add(usize::from(point.y - inner.y));
    if state.skill_reader_open {
        if row != 0 {
            return false;
        }
    } else if (1..=skills::entries().len()).contains(&row) {
        state.selected_skill = row - 1;
    } else {
        return false;
    }
    state.focus = PaneId::GateMonitor;
    switch_reader(state, !state.skill_reader_open, usize::from(inner.height));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use nit_core::Buffer;

    #[test]
    fn skill_browser_keeps_selection_visible_and_reads_only_when_focused() {
        let mut state = AppState::new(
            ".".into(),
            Buffer::empty("file", None),
            Buffer::empty("notes", None),
        );
        state.gate_monitor_sub_view = GateMonitorSubView::Skills;
        let area = Rect::new(5, 6, 26, 5);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        state.focus = PaneId::Editor;
        assert!(!handle_key(&mut state, key(KeyCode::Char('j')), area));
        state.focus = PaneId::GateMonitor;
        assert!(handle_key(&mut state, key(KeyCode::End), area));
        let index = skills::entries().len() - 1;
        assert_eq!(state.selected_skill, index);
        let last_visible = Position::new(area.x + 2, area.bottom() - 2);
        assert!(click(&mut state, area, last_visible));
        assert!(state.skill_reader_open);
        let body = lines(&state, &Theme::default(), 24);
        assert!(body.len() > 4);
        assert!(body.iter().all(|line| line.width() <= 24));
        state.gate_monitor_last_max_scroll = body.len() - 3;
        assert!(handle_key(&mut state, key(KeyCode::PageDown), area));
        assert_eq!(state.gate_monitor_scroll, 3);
        assert!(handle_key(&mut state, key(KeyCode::Esc), area));
        assert!(!state.skill_reader_open);
        assert_eq!(state.selected_skill, index);
        assert!(state.gate_monitor_scroll > 0);
        assert!(handle_key(&mut state, key(KeyCode::Enter), area));
        assert!(click(
            &mut state,
            area,
            Position::new(area.x + 1, area.y + 1)
        ));
        assert!(!state.skill_reader_open);
    }
}
