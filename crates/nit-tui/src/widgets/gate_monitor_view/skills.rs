use std::{cell::RefCell, rc::Rc};

use crossterm::event::{KeyCode, KeyEvent};
use nit_core::{skills, AppKind, AppState, GateMonitorSubView, PaneId, UiSelectionPane};
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::{theme::Theme, widgets::text_selection::apply_ui_selection};

mod attach;
mod catalog;
mod document;

pub(crate) fn active(state: &AppState) -> bool {
    state.app_kind == AppKind::Gol && state.gate_monitor_sub_view == GateMonitorSubView::Skills
}

pub(crate) fn focused(state: &AppState) -> bool {
    active(state)
        && state.focus == PaneId::GateMonitor
        && state.command_line.is_none()
        && state.prompt.is_none()
        && state.search_prompt.is_none()
        && !state.show_help
        && !state.fuzzy_search.open
        && !state.rule_picker.open
        && !state.protocol_picker.open
        && state.definition_popup.is_none()
        && !state.agents.artifacts_popup_open
        && !state.agents.global_archive_open
        && !state.agents.show_multiway_popup
        && !state.show_substrate_overlay
}

struct Regions {
    catalog: Rect,
    header: Rect,
    body: Rect,
    help: Rect,
    divider: Rect,
    filter: Rect,
}

fn regions(inner: Rect, state: &AppState) -> Regions {
    let reader = state.skill_reader_open;
    let show_filter = !reader && (state.skill_filter_editing || !state.skill_filter.is_empty());
    let filter = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        u16::from(show_filter && inner.height > 0),
    );
    let inner = Rect::new(
        inner.x,
        filter.bottom(),
        inner.width,
        inner.height - filter.height,
    );
    let help_height = u16::from(inner.height >= 4);
    let content = Rect::new(inner.x, inner.y, inner.width, inner.height - help_height);
    let help = Rect::new(inner.x, content.bottom(), inner.width, help_height);
    let (catalog, preview, divider) = if reader {
        (Rect::default(), content, Rect::default())
    } else if inner.width >= 60 && inner.height >= 8 {
        let width = (inner.width / 3).clamp(19, 32);
        (
            Rect::new(inner.x, inner.y, width, content.height),
            Rect::new(
                inner.x + width + 1,
                inner.y,
                inner.width - width - 1,
                content.height,
            ),
            Rect::new(inner.x + width, inner.y, 1, content.height),
        )
    } else {
        let height = if content.height >= 5 {
            (content.height / 2).clamp(3, 8)
        } else {
            content.height
        };
        (
            Rect::new(inner.x, inner.y, inner.width, height),
            Rect::new(
                inner.x,
                inner.y + height,
                inner.width,
                content.height - height,
            ),
            Rect::default(),
        )
    };
    let header = Rect::new(preview.x, preview.y, preview.width, preview.height.min(1));
    let body = Rect::new(
        preview.x,
        header.bottom(),
        preview.width,
        preview.height - header.height,
    );
    Regions {
        catalog,
        header,
        body,
        help,
        divider,
        filter,
    }
}

fn inner(area: Rect) -> Rect {
    Block::default().borders(Borders::ALL).inner(area)
}

pub(crate) fn document_area(state: &AppState, area: Rect) -> Rect {
    regions(inner(area), state).body
}

fn entry(state: &AppState) -> &'static skills::Skill {
    let entries = skills::definitions();
    &entries[state.selected_skill.min(entries.len() - 1)]
}

fn group_color(group: &str, theme: &Theme) -> Color {
    match group {
        "Code structure" => theme.hl.r#type,
        "Swarm workflow" => theme.hl.macro_token,
        "Research" => theme.hl.string,
        "Workspace" => theme.hl.link,
        _ => theme.foreground,
    }
}

struct DocumentCache {
    id: &'static str,
    width: usize,
    colors: [Color; 3],
    rows: Rc<[Line<'static>]>,
}

thread_local! {
    static DOCUMENT: RefCell<Option<DocumentCache>> = const { RefCell::new(None) };
}

fn document_rows(state: &AppState, theme: &Theme, width: usize) -> Rc<[Line<'static>]> {
    let skill = entry(state);
    let width = width.max(1);
    let heading = group_color(skill.group, theme);
    let colors = [heading, theme.foreground, theme.hl.string];
    DOCUMENT.with_borrow_mut(|cache| {
        if let Some(hit) = cache
            .as_ref()
            .filter(|c| c.id == skill.name && c.width == width && c.colors == colors)
        {
            return Rc::clone(&hit.rows);
        }
        let mut rows: Vec<_> =
            crate::widgets::picker_utils::wrap_text(&format!("nit skills {}", skill.name), width)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(theme.hl.string)))
                .collect();
        rows.extend(document::lines(skill.body, width, theme, heading));
        let rows = Rc::from(rows);
        *cache = Some(DocumentCache {
            id: skill.name,
            width,
            colors,
            rows: Rc::clone(&rows),
        });
        rows
    })
}

pub(super) fn lines(state: &AppState, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if catalog::matches(state).is_empty() {
        Vec::new()
    } else {
        document_rows(state, theme, width).to_vec()
    }
}

fn top(state: &AppState, area: Rect, indices: &[usize]) -> usize {
    let row = catalog::rows(indices)
        .position(|(i, _)| i == Some(state.selected_skill))
        .unwrap_or(0);
    (row + 1).saturating_sub(usize::from(area.height))
}

fn select(state: &mut AppState, index: usize) {
    state.selected_skill = index.min(skills::definitions().len() - 1);
    state.gate_monitor_scroll = 0;
    state.ui_selection = None;
}

pub(super) fn render(frame: &mut Frame, inner: Rect, state: &mut AppState, theme: &Theme) {
    let r = regions(inner, state);
    let accent = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let regular = Style::default().fg(theme.foreground);
    let muted = Style::default().fg(theme.muted);
    let has_focus = focused(state);
    let selected = regular
        .bg(if has_focus {
            theme.selection_bg
        } else {
            theme.cursor_line_bg
        })
        .add_modifier(Modifier::BOLD);
    let indices = catalog::matches(state);
    for (area, label, action) in footer_actions(state, r.help, !indices.is_empty()) {
        let style = if action == FooterAction::Use {
            accent.bg(theme.selection_bg)
        } else {
            muted
        };
        frame.render_widget(Paragraph::new(label).style(style), area);
    }
    if r.filter.height > 0 {
        let mark = if state.skill_filter_editing {
            "▏"
        } else {
            ""
        };
        let count = format!("{} found", indices.len());
        let room = r.filter.width.saturating_sub(count.len() as u16 + 1);
        let label = crate::widgets::text_utils::truncate_with_ellipsis(
            &format!("/ {}{mark}", state.skill_filter),
            usize::from(room),
        );
        frame.render_widget(
            Paragraph::new(format!(
                "{label:<width$} {count}",
                width = usize::from(room)
            ))
            .style(accent.bg(theme.list_background())),
            r.filter,
        );
    }
    if indices.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching skills").style(regular.bg(theme.list_background())),
            r.catalog,
        );
        state.gate_monitor_last_max_scroll = 0;
        return;
    }
    let mut rows = Vec::new();
    for (index, name) in catalog::rows(&indices) {
        if let Some(i) = index {
            let (marker, style) = if i == state.selected_skill {
                ("›", selected)
            } else {
                (" ", regular)
            };
            let name = crate::widgets::text_utils::truncate_with_ellipsis(
                name,
                usize::from(r.catalog.width.saturating_sub(2)),
            );
            let marker_style = if i == state.selected_skill && has_focus {
                style.fg(theme.border_focused)
            } else {
                style
            };
            rows.push(Line::from(vec![
                Span::styled(format!("{marker} "), marker_style),
                Span::styled(
                    format!(
                        "{name:<width$}",
                        width = usize::from(r.catalog.width.saturating_sub(2))
                    ),
                    style,
                ),
            ]));
        } else {
            rows.push(Line::styled(name, accent));
        }
    }
    frame.render_widget(
        Paragraph::new(rows)
            .style(regular.bg(theme.list_background()))
            .scroll((top(state, r.catalog, &indices) as u16, 0)),
        r.catalog,
    );
    if r.divider.width > 0 {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("│", muted);
                usize::from(r.divider.height)
            ]),
            r.divider,
        );
    }
    let skill = entry(state);
    let heading = Style::default()
        .fg(group_color(skill.group, theme))
        .add_modifier(Modifier::BOLD);
    let link = Style::default()
        .fg(theme.hl.link)
        .add_modifier(Modifier::UNDERLINED);
    let available = usize::from(r.header.width);
    let action_space = if state.skill_reader_open { 9 } else { 8 };
    let name = crate::widgets::text_utils::truncate_with_ellipsis(
        skill.title,
        available.saturating_sub(action_space),
    );
    let header = if state.skill_reader_open {
        let mut spans = vec![
            Span::styled("‹ Back", link),
            Span::raw("   "),
            Span::styled(name, heading),
        ];
        if spans.iter().map(Span::width).sum::<usize>() + 3 + Span::raw(skill.group).width()
            <= usize::from(r.header.width)
        {
            spans.push(Span::styled(" · ", muted));
            spans.push(Span::styled(skill.group, heading));
        }
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::styled(name, heading),
            Span::raw("  "),
            Span::styled("↗ Open", link),
        ])
    };
    frame.render_widget(Paragraph::new(header), r.header);
    if !r.body.is_empty() {
        let body = document_rows(state, theme, usize::from(r.body.width));
        state.gate_monitor_last_max_scroll = body.len().saturating_sub(usize::from(r.body.height));
        state.gate_monitor_scroll = state
            .gate_monitor_scroll
            .min(state.gate_monitor_last_max_scroll);
        let body = apply_ui_selection(
            body[state.gate_monitor_scroll..]
                .iter()
                .take(usize::from(r.body.height))
                .cloned()
                .collect(),
            state.ui_selection.as_ref(),
            UiSelectionPane::GateMonitor,
            theme.selection_bg,
            state.gate_monitor_scroll,
        );
        frame.render_widget(Paragraph::new(body).style(regular), r.body);
    } else {
        state.gate_monitor_last_max_scroll = 0;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FooterAction {
    Use,
    Read,
    Back,
    Filter,
    Done,
    Clear,
    Hint,
}

fn footer_actions(
    state: &AppState,
    area: Rect,
    has_matches: bool,
) -> Vec<(Rect, &'static str, FooterAction)> {
    use FooterAction::*;
    let mut actions = Vec::new();
    let mut x = area.x;
    let mut add = |label: &'static str, action| {
        let width = unicode_width::UnicodeWidthStr::width(label) as u16;
        if area.height > 0 && area.right().saturating_sub(x) >= width {
            actions.push((Rect::new(x, area.y, width, 1), label, action));
            x = x.saturating_add(width + 2);
        }
    };
    if has_matches {
        // In the filter field, 'a' is input. The pointer action remains available.
        add(
            if state.skill_filter_editing || area.width < 13 {
                "Use skill"
            } else {
                "[a] Use skill"
            },
            Use,
        );
    }
    if !has_matches {
        add("Esc clear filter", Clear);
    } else if state.skill_filter_editing {
        add("Enter done", Done);
        add("Esc clear", Clear);
    } else if state.skill_reader_open {
        add("Esc back", Back);
        add("↑↓ scroll", Hint);
    } else {
        add("↵ read", Read);
        add("/ filter", Filter);
        add("↑↓ choose", Hint);
    }
    actions
}

fn switch_reader(state: &mut AppState, open: bool) {
    state.skill_reader_open = open;
    select(state, state.selected_skill);
}

pub(crate) fn paste(state: &mut AppState, text: &str) -> bool {
    if !focused(state) || !state.skill_filter_editing {
        return false;
    }
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !state.skill_filter.ends_with(' ') {
                state.skill_filter.push(' ');
            }
        } else if !ch.is_control() {
            state.skill_filter.push(ch);
        }
    }
    catalog::reconcile(state);
    true
}

pub(crate) fn handle_key(state: &mut AppState, key: KeyEvent, area: Rect) -> bool {
    if !focused(state) || !(key.modifiers - crossterm::event::KeyModifiers::SHIFT).is_empty() {
        return false;
    }
    let page = usize::from(document_area(state, area).height.max(1));
    if state.skill_filter_editing {
        match key.code {
            KeyCode::Char(ch) if !ch.is_control() => state.skill_filter.push(ch),
            KeyCode::Backspace => {
                state.skill_filter.pop();
            }
            KeyCode::Esc => {
                state.skill_filter.clear();
                state.skill_filter_editing = false;
            }
            KeyCode::Enter => state.skill_filter_editing = false,
            KeyCode::Up | KeyCode::Down => {
                state.skill_filter_editing = false;
                catalog::step(state, if key.code == KeyCode::Down { 1 } else { -1 });
            }
            _ => return false,
        }
        catalog::reconcile(state);
        return true;
    }
    if key.code == KeyCode::Char('a') {
        if catalog::matches(state).contains(&state.selected_skill) {
            attach::attach(state, entry(state).name);
        }
        return true;
    }
    if state.skill_reader_open {
        let scroll = state.gate_monitor_scroll;
        match key.code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Left => switch_reader(state, false),
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
                    .saturating_add(page)
                    .min(state.gate_monitor_last_max_scroll)
            }
            KeyCode::PageUp => state.gate_monitor_scroll = scroll.saturating_sub(page),
            KeyCode::Home => state.gate_monitor_scroll = 0,
            KeyCode::End => state.gate_monitor_scroll = state.gate_monitor_last_max_scroll,
            _ => return false,
        }
    } else {
        match key.code {
            KeyCode::Char('/') => state.skill_filter_editing = true,
            KeyCode::Esc if !state.skill_filter.is_empty() => {
                state.skill_filter.clear();
                catalog::reconcile(state);
            }
            KeyCode::Enter => {
                if !catalog::matches(state).is_empty() {
                    switch_reader(state, true);
                }
            }
            KeyCode::Home => catalog::step(state, isize::MIN),
            KeyCode::End => catalog::step(state, isize::MAX),
            KeyCode::Up | KeyCode::Char('k') => catalog::step(state, -1),
            KeyCode::Down | KeyCode::Char('j') => catalog::step(state, 1),
            _ => return false,
        }
    }
    true
}

pub(crate) fn wheel(state: &mut AppState, area: Rect, point: Position, delta: i32) -> bool {
    if !active(state) {
        return false;
    }
    let r = regions(inner(area), state);
    if r.catalog.contains(point) {
        catalog::step(state, delta.signum() as isize);
    } else if r.body.contains(point) || r.header.contains(point) {
        state.gate_monitor_scroll = state
            .gate_monitor_scroll
            .saturating_add_signed(delta as isize)
            .min(state.gate_monitor_last_max_scroll);
    }
    true
}

pub(crate) fn click(state: &mut AppState, area: Rect, point: Position) -> bool {
    if !active(state) {
        return false;
    }
    let r = regions(inner(area), state);
    let indices = catalog::matches(state);
    if r.help.contains(point) {
        let hit = footer_actions(state, r.help, !indices.is_empty())
            .into_iter()
            .find(|(area, _, _)| area.contains(point));
        let Some((_, _, action)) = hit else {
            return false;
        };
        state.focus = PaneId::GateMonitor;
        match action {
            FooterAction::Use if indices.contains(&state.selected_skill) => {
                state.skill_filter_editing = false;
                attach::attach(state, entry(state).name);
            }
            FooterAction::Read => switch_reader(state, true),
            FooterAction::Back => switch_reader(state, false),
            FooterAction::Filter => state.skill_filter_editing = true,
            FooterAction::Done => state.skill_filter_editing = false,
            FooterAction::Clear => {
                state.skill_filter.clear();
                state.skill_filter_editing = false;
                catalog::reconcile(state);
            }
            _ => {}
        }
        return true;
    }
    if r.filter.contains(point) {
        state.skill_filter_editing = true;
    } else if r.header.contains(point) && !indices.is_empty() {
        state.skill_filter_editing = false;
        switch_reader(state, !state.skill_reader_open);
    } else if r.catalog.contains(point) {
        let row = top(state, r.catalog, &indices) + usize::from(point.y - r.catalog.y);
        if let Some((Some(index), _)) = catalog::rows(&indices).nth(row) {
            state.skill_filter_editing = false;
            select(state, index);
        }
    } else {
        return false;
    }
    state.focus = PaneId::GateMonitor;
    true
}

#[cfg(test)]
mod tests;
