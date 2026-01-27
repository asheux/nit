use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{actions::Action, AppState};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn handle_key(key: &KeyEvent, state: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc => {
            let _ = nit_core::apply_action(state, Action::CloseModal);
            true
        }
        KeyCode::Enter => {
            let _ = nit_core::apply_action(state, Action::ApplySelectedRuleFromPicker);
            true
        }
        KeyCode::Backspace => {
            state.rule_picker.query.pop();
            state.rule_picker.selected = 0;
            true
        }
        KeyCode::Up => {
            let len = state.rule_catalog.filter_indices(&state.rule_picker.query).len();
            if len > 0 {
                if state.rule_picker.selected == 0 {
                    state.rule_picker.selected = len - 1;
                } else {
                    state.rule_picker.selected -= 1;
                }
            }
            true
        }
        KeyCode::Down => {
            let len = state.rule_catalog.filter_indices(&state.rule_picker.query).len();
            if len > 0 {
                state.rule_picker.selected = (state.rule_picker.selected + 1) % len;
            }
            true
        }
        KeyCode::PageUp => {
            let len = state.rule_catalog.filter_indices(&state.rule_picker.query).len();
            if len > 0 {
                state.rule_picker.selected = state.rule_picker.selected.saturating_sub(6);
            }
            true
        }
        KeyCode::PageDown => {
            let len = state.rule_catalog.filter_indices(&state.rule_picker.query).len();
            if len > 0 {
                state.rule_picker.selected = (state.rule_picker.selected + 6).min(len - 1);
            }
            true
        }
        KeyCode::Home => {
            state.rule_picker.selected = 0;
            true
        }
        KeyCode::End => {
            let len = state.rule_catalog.filter_indices(&state.rule_picker.query).len();
            if len > 0 {
                state.rule_picker.selected = len - 1;
            }
            true
        }
        KeyCode::Char(ch) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                state.rule_picker.query.push(ch);
                state.rule_picker.selected = 0;
                return true;
            }
            false
        }
        _ => false,
    }
}

pub fn render(frame: &mut Frame, screen: Rect, state: &AppState, theme: &Theme) {
    let matches = state.rule_catalog.filter_indices(&state.rule_picker.query);
    let area = fixed_rect(screen);
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    let popup_bg = theme.selection_bg;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            "RULE PICKER",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(popup_bg));
    let inner = block.inner(area);
    if inner.height < 3 {
        return;
    }
    frame.render_widget(block, area);
    let list_height = inner.height.saturating_sub(2);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(list_height),
            Constraint::Length(1),
        ])
        .split(inner);

    let label_style = Style::default()
        .fg(theme.title)
        .bg(popup_bg)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground).bg(popup_bg);
    let filter_line = Line::from(vec![
        Span::styled("Filter: ", label_style),
        Span::styled(state.rule_picker.query.clone(), value_style),
    ]);
    let filter = Paragraph::new(filter_line).style(Style::default().bg(popup_bg));
    frame.render_widget(filter, chunks[0]);

    if matches.is_empty() {
        let empty = Paragraph::new("No matching rules")
            .style(Style::default().fg(theme.warning).bg(popup_bg));
        frame.render_widget(empty, chunks[1]);
    } else {
        let mut items = Vec::with_capacity(matches.len());
        let max_width = chunks[1].width as usize;
        let max_item_width = max_width.saturating_sub(2);
        for idx in matches.iter() {
            if let Some(rule) = state.rule_catalog.get(*idx) {
                let raw = format_rule_line_raw(rule);
                items.push(ListItem::new(truncate_text(&raw, max_item_width)));
            }
        }
        let mut list_state = ListState::default();
        let selected = state.rule_picker.selected.min(items.len().saturating_sub(1));
        list_state.select(Some(selected));
        let item_style = Style::default()
            .fg(ratatui::style::Color::Gray)
            .bg(popup_bg);
        let list = List::new(items)
            .style(item_style)
            .highlight_style(
                Style::default()
                    .fg(theme.foreground)
                    .bg(theme.selection_bg)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        frame.render_stateful_widget(list, chunks[1], &mut list_state);
    }

    let footer_line = Line::from(vec![
        Span::styled(
            "Enter apply",
            Style::default().fg(theme.accent).add_modifier(Modifier::DIM),
        ),
        Span::styled(" | ", Style::default().fg(theme.border).add_modifier(Modifier::DIM)),
        Span::styled(
            "Esc cancel",
            Style::default().fg(theme.accent).add_modifier(Modifier::DIM),
        ),
        Span::styled(" | ", Style::default().fg(theme.border).add_modifier(Modifier::DIM)),
        Span::styled(
            "Type to filter",
            Style::default().fg(theme.border).add_modifier(Modifier::DIM),
        ),
    ]);
    let footer = Paragraph::new(footer_line).style(Style::default().bg(popup_bg));
    frame.render_widget(footer, chunks[2]);
}

fn centered_rect_px(screen: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(screen.width);
    let h = height.min(screen.height);
    let x = screen.x + screen.width.saturating_sub(w) / 2;
    let y = screen.y + screen.height.saturating_sub(h) / 2;
    Rect { x, y, width: w, height: h }
}

fn fixed_rect(screen: Rect) -> Rect {
    let max_w = screen.width.saturating_sub(4).max(10);
    let max_h = screen.height.saturating_sub(4).max(6);
    let width = 80u16.min(max_w);
    let height = 22u16.min(max_h);
    centered_rect_px(screen, width, height)
}

fn format_rule_line_raw(rule: &nit_core::NamedRule) -> String {
    let mut line = format!("{} | {} | {}", rule.id, rule.name, rule.rule);
    if !rule.description.is_empty() {
        line.push_str(" — ");
        line.push_str(&rule.description);
    }
    line
}

fn truncate_text(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count >= max {
            break;
        }
        out.push(ch);
        count += 1;
    }
    if text.chars().count() > max && max > 3 {
        let trimmed: String = out.chars().take(max - 3).collect();
        return format!("{trimmed}...");
    }
    out
}
