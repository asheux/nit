//! Popup picker for Substrate protocol presets (plus a "Custom..." slot that
//! parses a free-form spec via `nit_core::parse_protocol_spec`). Opened via
//! the Substrate menu; Esc cancels, Enter applies.

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
use crate::widgets::picker_utils::{centered_rect_px, truncate_text};

const POPUP_MAX_WIDTH: u16 = 96;
const POPUP_MAX_HEIGHT: u16 = 24;
const PAGE_JUMP: usize = 6;
/// Rows reserved below the preset list for the detail line, the custom input
/// row, and the footer shortcuts row.
const RESERVED_DETAIL_ROWS: u16 = 3;
const MIN_INNER_HEIGHT: u16 = 4;

/// Handle a key event for the protocol picker. Returns true if the event was
/// consumed (including typed characters that mutate the custom-input buffer).
pub fn handle_key(key: &KeyEvent, state: &mut AppState) -> bool {
    let custom_slot = nit_core::builtin_protocols(&state.rule_catalog).len();
    let option_count = custom_slot + 1;
    let last_idx = option_count.saturating_sub(1);
    let cursor = state.protocol_picker.selected;
    match key.code {
        KeyCode::Esc => {
            let _ = nit_core::apply_action(state, Action::CloseModal);
        }
        KeyCode::Enter => {
            let _ = nit_core::apply_action(state, Action::ApplySelectedProtocolFromPicker);
        }
        KeyCode::Up => state.protocol_picker.selected = step_up(cursor, option_count),
        KeyCode::Down => state.protocol_picker.selected = step_down(cursor, option_count),
        KeyCode::PageUp => state.protocol_picker.selected = cursor.saturating_sub(PAGE_JUMP),
        KeyCode::PageDown => state.protocol_picker.selected = (cursor + PAGE_JUMP).min(last_idx),
        KeyCode::Home => state.protocol_picker.selected = 0,
        KeyCode::End => state.protocol_picker.selected = last_idx,
        KeyCode::Backspace => {
            state.protocol_picker.selected = custom_slot;
            state.protocol_picker.custom_input.pop();
            update_custom_preview(state);
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.protocol_picker.selected = custom_slot;
            state.protocol_picker.custom_input.push(ch);
            update_custom_preview(state);
        }
        _ => return false,
    }
    true
}

fn step_up(cursor: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    cursor.checked_sub(1).unwrap_or(total - 1)
}

fn step_down(cursor: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    (cursor + 1) % total
}

/// Render the protocol picker popup over `screen`. The popup auto-centers and
/// clamps to `POPUP_MAX_WIDTH`/`POPUP_MAX_HEIGHT`; too-small terminals bail
/// early so the caller sees nothing drawn rather than a clipped frame.
pub fn render(frame: &mut Frame, screen: Rect, state: &AppState, theme: &Theme) {
    let area = popup_rect(screen);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let popup_bg = theme.selection_bg;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            "PROTOCOL PICKER",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(popup_bg));
    let inner = block.inner(area);
    if inner.height < MIN_INNER_HEIGHT {
        return;
    }
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let list_height = inner.height.saturating_sub(RESERVED_DETAIL_ROWS);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(list_height),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let presets = nit_core::builtin_protocols(&state.rule_catalog);
    let list_width = chunks[0].width as usize;
    let items: Vec<ListItem<'static>> = presets
        .iter()
        .map(|preset| {
            let line = format!("{} - {}", preset.name, preset.description);
            ListItem::new(Line::from(truncate_text(&line, list_width)))
        })
        .chain(std::iter::once(ListItem::new(Line::from(truncate_text(
            "Custom...",
            list_width,
        )))))
        .collect();

    let selected = state
        .protocol_picker
        .selected
        .min(items.len().saturating_sub(1));
    let mut list_state = ListState::default();
    list_state.select(Some(selected));
    let list = List::new(items)
        .style(Style::default().fg(ratatui::style::Color::Gray).bg(popup_bg))
        .highlight_style(
            Style::default()
                .fg(theme.foreground)
                .bg(theme.selection_bg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    render_detail(frame, chunks[1], &presets, state, theme, popup_bg, selected);
    render_custom_input(
        frame,
        chunks[2],
        state,
        theme,
        popup_bg,
        selected >= presets.len(),
    );
    render_footer(frame, chunks[3], theme, popup_bg);
}

fn render_detail(
    frame: &mut Frame,
    area: Rect,
    presets: &[nit_core::ProtocolPreset],
    state: &AppState,
    theme: &Theme,
    popup_bg: ratatui::style::Color,
    selected: usize,
) {
    let detail = protocol_detail_line(presets, state, selected);
    let detail_style = match detail.kind {
        DetailKind::Warn => Style::default().fg(theme.warning).bg(popup_bg),
        DetailKind::Dim => Style::default()
            .fg(theme.border)
            .bg(popup_bg)
            .add_modifier(Modifier::DIM),
    };
    let detail_width = area.width as usize;
    let detail_text = truncate_text(&detail.text, detail_width.saturating_sub(1));
    frame.render_widget(Paragraph::new(detail_text).style(detail_style), area);
}

fn render_custom_input(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    popup_bg: ratatui::style::Color,
    is_custom_selected: bool,
) {
    let input_label = Style::default()
        .fg(theme.title)
        .bg(popup_bg)
        .add_modifier(Modifier::DIM);
    let input_value = if is_custom_selected {
        Style::default().fg(theme.foreground).bg(popup_bg)
    } else {
        Style::default().fg(theme.border).bg(popup_bg)
    };
    let prefix = "Custom: ";
    let value_width = (area.width as usize).saturating_sub(prefix.chars().count());
    let value_text = truncate_text(&state.protocol_picker.custom_input, value_width);
    let input_line = Line::from(vec![
        Span::styled(prefix, input_label),
        Span::styled(value_text, input_value),
    ]);
    frame.render_widget(
        Paragraph::new(input_line).style(Style::default().bg(popup_bg)),
        area,
    );
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
        Span::styled("Type to edit custom", border_dim),
    ]);
    frame.render_widget(
        Paragraph::new(footer_line).style(Style::default().bg(popup_bg)),
        area,
    );
}

fn update_custom_preview(state: &mut AppState) {
    let trimmed = state.protocol_picker.custom_input.trim();
    if trimmed.is_empty() {
        state.protocol_picker.custom_preview = None;
        state.protocol_picker.custom_error = None;
        return;
    }
    match nit_core::parse_protocol_spec(trimmed, &state.rule_catalog) {
        Ok(protocol) => {
            state.protocol_picker.custom_preview = Some(protocol.canonical_string());
            state.protocol_picker.custom_error = None;
        }
        Err(err) => {
            state.protocol_picker.custom_preview = None;
            state.protocol_picker.custom_error = Some(err);
        }
    }
}

fn popup_rect(screen: Rect) -> Rect {
    let max_w = screen.width.saturating_sub(4).max(10);
    let max_h = screen.height.saturating_sub(4).max(6);
    centered_rect_px(screen, POPUP_MAX_WIDTH.min(max_w), POPUP_MAX_HEIGHT.min(max_h))
}

struct DetailLine {
    text: String,
    kind: DetailKind,
}

enum DetailKind {
    Dim,
    Warn,
}

fn protocol_detail_line(
    presets: &[nit_core::ProtocolPreset],
    state: &AppState,
    selected: usize,
) -> DetailLine {
    if selected < presets.len() {
        let preset = &presets[selected];
        return DetailLine {
            text: format!(
                "{} - {}",
                preset.mode.canonical_string(),
                preset.description
            ),
            kind: DetailKind::Dim,
        };
    }
    if let Some(err) = state.protocol_picker.custom_error.as_ref() {
        return DetailLine {
            text: format!("Invalid: {err}"),
            kind: DetailKind::Warn,
        };
    }
    if let Some(preview) = state.protocol_picker.custom_preview.as_ref() {
        return DetailLine {
            text: format!("Preview: {preview}"),
            kind: DetailKind::Dim,
        };
    }
    DetailLine {
        text: "Custom protocol: type rule schedule".into(),
        kind: DetailKind::Dim,
    }
}
