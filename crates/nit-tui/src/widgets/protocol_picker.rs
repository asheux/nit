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
    let presets = nit_core::builtin_protocols(&state.rule_catalog);
    let total = presets.len().saturating_add(1);
    let custom_idx = presets.len();
    match key.code {
        KeyCode::Esc => {
            let _ = nit_core::apply_action(state, Action::CloseModal);
            true
        }
        KeyCode::Enter => {
            let _ = nit_core::apply_action(state, Action::ApplySelectedProtocolFromPicker);
            true
        }
        KeyCode::Up => {
            if total > 0 {
                if state.protocol_picker.selected == 0 {
                    state.protocol_picker.selected = total - 1;
                } else {
                    state.protocol_picker.selected -= 1;
                }
            }
            true
        }
        KeyCode::Down => {
            if total > 0 {
                state.protocol_picker.selected = (state.protocol_picker.selected + 1) % total;
            }
            true
        }
        KeyCode::PageUp => {
            if total > 0 {
                state.protocol_picker.selected = state.protocol_picker.selected.saturating_sub(6);
            }
            true
        }
        KeyCode::PageDown => {
            if total > 0 {
                state.protocol_picker.selected =
                    (state.protocol_picker.selected + 6).min(total - 1);
            }
            true
        }
        KeyCode::Home => {
            state.protocol_picker.selected = 0;
            true
        }
        KeyCode::End => {
            if total > 0 {
                state.protocol_picker.selected = total - 1;
            }
            true
        }
        KeyCode::Backspace => {
            state.protocol_picker.selected = custom_idx;
            state.protocol_picker.custom_input.pop();
            update_custom_preview(state);
            true
        }
        KeyCode::Char(ch) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                state.protocol_picker.selected = custom_idx;
                state.protocol_picker.custom_input.push(ch);
                update_custom_preview(state);
                return true;
            }
            false
        }
        _ => false,
    }
}

pub fn render(frame: &mut Frame, screen: Rect, state: &AppState, theme: &Theme) {
    let presets = nit_core::builtin_protocols(&state.rule_catalog);
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
            "PROTOCOL PICKER",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(popup_bg));
    let inner = block.inner(area);
    if inner.height < 4 {
        return;
    }
    frame.render_widget(block, area);
    let list_height = inner.height.saturating_sub(3);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(list_height),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let list_width = chunks[0].width as usize;
    let mut items = Vec::with_capacity(presets.len().saturating_add(1));
    for preset in presets.iter() {
        let line = format!("{} - {}", preset.name, preset.description);
        items.push(ListItem::new(Line::from(truncate_text(&line, list_width))));
    }
    items.push(ListItem::new(Line::from(truncate_text(
        "Custom...",
        list_width,
    ))));
    let mut list_state = ListState::default();
    let selected = state
        .protocol_picker
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
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    let detail_width = chunks[1].width as usize;
    let detail = protocol_detail_line(&presets, state, selected);
    let detail_style = match detail.kind {
        DetailKind::Warn => Style::default().fg(theme.warning).bg(popup_bg),
        DetailKind::Dim => Style::default()
            .fg(theme.border)
            .bg(popup_bg)
            .add_modifier(Modifier::DIM),
    };
    let detail_text = truncate_text(&detail.text, detail_width.saturating_sub(1));
    frame.render_widget(Paragraph::new(detail_text).style(detail_style), chunks[1]);

    let input_label = Style::default()
        .fg(theme.title)
        .bg(popup_bg)
        .add_modifier(Modifier::DIM);
    let input_value = if selected >= presets.len() {
        Style::default().fg(theme.foreground).bg(popup_bg)
    } else {
        Style::default().fg(theme.border).bg(popup_bg)
    };
    let prefix = "Custom: ";
    let input_width = chunks[2].width as usize;
    let value_width = input_width.saturating_sub(prefix.chars().count());
    let value_text = truncate_text(&state.protocol_picker.custom_input, value_width);
    let input_line = Line::from(vec![
        Span::styled(prefix, input_label),
        Span::styled(value_text, input_value),
    ]);
    let input = Paragraph::new(input_line).style(Style::default().bg(popup_bg));
    frame.render_widget(input, chunks[2]);

    let footer_line = Line::from(vec![
        Span::styled(
            "Enter apply",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            " | ",
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            "Esc cancel",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            " | ",
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            "Type to edit custom",
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ),
    ]);
    let footer = Paragraph::new(footer_line).style(Style::default().bg(popup_bg));
    frame.render_widget(footer, chunks[3]);
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

fn centered_rect_px(screen: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(screen.width);
    let h = height.min(screen.height);
    let x = screen.x + screen.width.saturating_sub(w) / 2;
    let y = screen.y + screen.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn fixed_rect(screen: Rect) -> Rect {
    let max_w = screen.width.saturating_sub(4).max(10);
    let max_h = screen.height.saturating_sub(4).max(6);
    let width = 96u16.min(max_w);
    let height = 24u16.min(max_h);
    centered_rect_px(screen, width, height)
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
        let text = format!(
            "{} - {}",
            preset.mode.canonical_string(),
            preset.description
        );
        return DetailLine {
            text,
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
