//! Popup picker for Substrate rule catalog entries with fuzzy filter. Opened
//! via the Substrate menu; Esc cancels, Enter applies. Typed characters live
//! in `rule_picker.query` and feed `rule_catalog.filter_indices`.

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
use crate::widgets::picker_utils::{centered_rect_px, truncate_text, wrap_text};

const POPUP_MAX_WIDTH: u16 = 101;
const POPUP_MAX_HEIGHT: u16 = 32;
const PAGE_JUMP: usize = 6;
/// Rows reserved outside the matches list: filter row + detail row + footer.
const RESERVED_CHROME_ROWS: u16 = 3;
const MIN_INNER_HEIGHT: u16 = 4;
/// Minimum width for the ASCII-bordered rule card. Below this the box frame
/// would not fit, so `format_rule_box_lines` falls back to a single line.
const BOX_MIN_WIDTH: usize = 4;

/// Handle a key event for the rule picker. Returns true if the event was
/// consumed (typed characters mutate `rule_picker.query` and reset selection).
pub fn handle_key(key: &KeyEvent, state: &mut AppState) -> bool {
    let len = filtered_len(state);
    let cur = state.rule_picker.selected;
    match key.code {
        KeyCode::Esc => {
            let _ = nit_core::apply_action(state, Action::CloseModal);
        }
        KeyCode::Enter => {
            let _ = nit_core::apply_action(state, Action::ApplySelectedRuleFromPicker);
        }
        KeyCode::Backspace => {
            state.rule_picker.query.pop();
            state.rule_picker.selected = 0;
        }
        KeyCode::Up if len > 0 => {
            state.rule_picker.selected = if cur == 0 { len - 1 } else { cur - 1 };
        }
        KeyCode::Down if len > 0 => state.rule_picker.selected = (cur + 1) % len,
        KeyCode::PageUp if len > 0 => state.rule_picker.selected = cur.saturating_sub(PAGE_JUMP),
        KeyCode::PageDown if len > 0 => {
            state.rule_picker.selected = (cur + PAGE_JUMP).min(len - 1);
        }
        KeyCode::Home => state.rule_picker.selected = 0,
        KeyCode::End if len > 0 => state.rule_picker.selected = len - 1,
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.rule_picker.query.push(ch);
            state.rule_picker.selected = 0;
        }
        KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown | KeyCode::End => {}
        _ => return false,
    }
    true
}

fn filtered_len(state: &AppState) -> usize {
    state
        .rule_catalog
        .filter_indices(&state.rule_picker.query)
        .len()
}

/// Render the rule picker popup. Centers over `screen`, clamps to
/// `POPUP_MAX_WIDTH`/`POPUP_MAX_HEIGHT`, and bails on too-small inner areas so
/// callers get no partial frame.
pub fn render(frame: &mut Frame, screen: Rect, state: &AppState, theme: &Theme) {
    let matches = state.rule_catalog.filter_indices(&state.rule_picker.query);
    let area = popup_rect(screen);
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
    if inner.height < MIN_INNER_HEIGHT {
        return;
    }
    frame.render_widget(block, area);
    let list_height = inner.height.saturating_sub(RESERVED_CHROME_ROWS);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(list_height),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    render_filter(frame, chunks[0], state, theme, popup_bg);
    render_matches(frame, chunks[1], &matches, state, theme, popup_bg);
    render_detail(frame, chunks[2], &matches, state, theme, popup_bg);
    render_footer(frame, chunks[3], theme, popup_bg);
}

fn render_filter(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    popup_bg: ratatui::style::Color,
) {
    let label_style = Style::default()
        .fg(theme.title)
        .bg(popup_bg)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground).bg(popup_bg);
    let filter_line = Line::from(vec![
        Span::styled("Filter: ", label_style),
        Span::styled(state.rule_picker.query.clone(), value_style),
    ]);
    frame.render_widget(
        Paragraph::new(filter_line).style(Style::default().bg(popup_bg)),
        area,
    );
}

fn render_matches(
    frame: &mut Frame,
    area: Rect,
    matches: &[usize],
    state: &AppState,
    theme: &Theme,
    popup_bg: ratatui::style::Color,
) {
    if matches.is_empty() {
        let empty = Paragraph::new("No matching rules")
            .style(Style::default().fg(theme.warning).bg(popup_bg));
        frame.render_widget(empty, area);
        return;
    }
    let max_item_width = area.width as usize;
    let items: Vec<ListItem<'static>> = matches
        .iter()
        .filter_map(|idx| state.rule_catalog.get(*idx))
        .map(|rule| ListItem::new(format_rule_box_lines(rule, max_item_width)))
        .collect();
    let mut list_state = ListState::default();
    let selected = state
        .rule_picker
        .selected
        .min(items.len().saturating_sub(1));
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
        .highlight_symbol("");
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_detail(
    frame: &mut Frame,
    area: Rect,
    matches: &[usize],
    state: &AppState,
    theme: &Theme,
    popup_bg: ratatui::style::Color,
) {
    let detail_text = selected_rule_detail(matches, state);
    let detail_style = if detail_text.warning.is_some() {
        Style::default().fg(theme.warning).bg(popup_bg)
    } else {
        Style::default()
            .fg(theme.border)
            .bg(popup_bg)
            .add_modifier(Modifier::DIM)
    };
    let detail_line = detail_text
        .line
        .map(|line| truncate_text(&line, (area.width as usize).saturating_sub(1)))
        .unwrap_or_else(|| "No rule selected".to_string());
    frame.render_widget(Paragraph::new(detail_line).style(detail_style), area);
}

fn render_footer(frame: &mut Frame, area: Rect, theme: &Theme, popup_bg: ratatui::style::Color) {
    let accent_dim = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::DIM);
    let border_dim = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let footer_line = Line::from(vec![
        Span::styled("Enter apply", accent_dim),
        Span::styled(" | ", border_dim),
        Span::styled("Esc cancel", accent_dim),
        Span::styled(" | ", border_dim),
        Span::styled("Type to filter", border_dim),
    ]);
    frame.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(popup_bg)),
        area,
    );
}

fn popup_rect(screen: Rect) -> Rect {
    let max_w = screen.width.saturating_sub(4).max(10);
    let max_h = screen.height.saturating_sub(4).max(6);
    centered_rect_px(screen, POPUP_MAX_WIDTH.min(max_w), POPUP_MAX_HEIGHT.min(max_h))
}

fn format_rule_box_lines(rule: &nit_core::NamedRule, width: usize) -> Vec<Line<'static>> {
    let rule_text = format!("{} | {} | {}", rule.id, rule.name, rule.rulestring);
    if width < BOX_MIN_WIDTH {
        return vec![Line::from(truncate_text(&rule_text, width))];
    }
    let inner = width.saturating_sub(BOX_MIN_WIDTH);
    let mut lines = Vec::with_capacity(4);
    lines.push(Line::from(box_border_line(width)));
    for line in wrap_text(&rule_text, inner) {
        lines.push(Line::from(box_content_line(&line, width)));
    }
    for line in wrap_text(&rule.description, inner) {
        lines.push(Line::from(box_content_line(&line, width)));
    }
    lines.push(Line::from(box_border_line(width)));
    lines
}

fn box_border_line(width: usize) -> String {
    let inner = width.saturating_sub(2);
    let mut line = String::with_capacity(width);
    line.push('+');
    line.push_str(&"-".repeat(inner));
    line.push('+');
    line
}

fn box_content_line(text: &str, width: usize) -> String {
    if width < BOX_MIN_WIDTH {
        return truncate_text(text, width);
    }
    let inner = width.saturating_sub(BOX_MIN_WIDTH);
    let trimmed = truncate_text(text, inner);
    let padding = inner.saturating_sub(trimmed.chars().count());
    let mut line = String::with_capacity(width);
    line.push('|');
    line.push(' ');
    line.push_str(&trimmed);
    if padding > 0 {
        line.push_str(&" ".repeat(padding));
    }
    line.push(' ');
    line.push('|');
    line
}

struct RuleDetailLine {
    line: Option<String>,
    warning: Option<String>,
}

fn selected_rule_detail(matches: &[usize], state: &AppState) -> RuleDetailLine {
    if matches.is_empty() {
        return RuleDetailLine {
            line: None,
            warning: None,
        };
    }
    let selected = state
        .rule_picker
        .selected
        .min(matches.len().saturating_sub(1));
    let Some(rule) = state.rule_catalog.get(matches[selected]) else {
        return RuleDetailLine {
            line: None,
            warning: None,
        };
    };
    let mut parts = vec![rule.rulestring.clone()];
    if !rule.description.is_empty() {
        parts.push(rule.description.clone());
    }
    if !rule.tags.is_empty() {
        parts.push(format!("tags: {}", rule.tags.join(", ")));
    }
    let mut line = parts.join(" — ");
    let warning = rule.warning().map(|w| w.to_string());
    if let Some(warn) = warning.as_deref() {
        line.push_str(" — WARN: ");
        line.push_str(warn);
    }
    RuleDetailLine {
        line: Some(line),
        warning,
    }
}
