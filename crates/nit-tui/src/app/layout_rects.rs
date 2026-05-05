use crate::widgets::fuzzy_search_popup;
use nit_core::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders};

pub(super) fn dynamic_popup_rect(screen: Rect, desired: (u16, u16)) -> Rect {
    let max_w = screen.width.saturating_sub(4).max(10);
    let max_h = screen.height.saturating_sub(2).max(5);
    let width = desired.0.min(max_w);
    let height = desired.1.min(max_h);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((screen.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(screen)[1];
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((screen.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical)[1]
}

pub(super) fn fuzzy_popup_size(screen: Rect, _state: &AppState) -> (u16, u16) {
    fuzzy_search_popup::preferred_size(screen)
}

pub(super) fn point_in_rect(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

pub(super) fn job_output_text_area(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    // Agent Ops layout: tabs + spacer + body + footer hints.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    chunks[2]
}

pub(super) fn agent_ops_tab_bar_area(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.min(1),
    }
}

pub(super) fn agent_ops_scratchpad_editor_area(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    // Agent Ops Scratchpad layout: tabs + editor body.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    chunks[1]
}

pub(super) fn popup_text_area(area: Rect) -> Rect {
    Block::default().borders(Borders::ALL).inner(area)
}
