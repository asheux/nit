//! Substrate inspector popup — shows signals / claims / assumptions as
//! switchable sub-tabs. Opens via F3 or the :substrate / :sub / :sig /
//! :claims / :assumptions / :asm commands. Esc to close.

use nit_core::{Action, AppState, SubstrateOverlayTab};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::{assumptions_view, claims_view, signals_view};

const TITLE_PREFIX: &str = " SUBSTRATE ";
const TAB_LABELS: &[(SubstrateOverlayTab, &str)] = &[
    (SubstrateOverlayTab::Signals, " SIGNALS "),
    (SubstrateOverlayTab::Claims, " CLAIMS "),
    (SubstrateOverlayTab::Assumptions, " ASSUMPTIONS "),
];

/// Preferred popup rect — content-appropriate, not full-screen. The table
/// needs ~105 cols to show every column; we cap width there, cap height at
/// ~28 rows, and bias smaller when the screen is small.
pub fn preferred_size(screen: Rect) -> Rect {
    let target_w = (screen.width.saturating_mul(75) / 100).max(96);
    let target_h = (screen.height.saturating_mul(55) / 100).max(18);
    let w = target_w.min(160).min(screen.width.saturating_sub(4));
    let h = target_h.min(32).min(screen.height.saturating_sub(4));
    let w = w.max(60);
    let h = h.max(12);
    let x = screen.x + (screen.width.saturating_sub(w)) / 2;
    let y = screen.y + (screen.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &mut AppState, theme: &Theme) {
    frame.render_widget(Clear, area);

    let mut title_spans: Vec<Span<'static>> = vec![Span::styled(
        TITLE_PREFIX,
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    )];
    for (tab, label) in TAB_LABELS {
        let active = *tab == state.substrate_overlay_tab;
        let style = if active {
            Style::default()
                .fg(theme.background)
                .bg(theme.title_focused)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::DIM)
        };
        title_spans.push(Span::styled(label.to_string(), style));
    }
    title_spans.push(Span::styled(
        "   F3/Esc close   Tab: switch   j/k: scroll",
        Style::default().add_modifier(Modifier::DIM),
    ));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_focused))
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .title(Line::from(title_spans));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    match state.substrate_overlay_tab {
        SubstrateOverlayTab::Signals => signals_view::render_body(frame, inner, state, theme),
        SubstrateOverlayTab::Claims => claims_view::render_body(frame, inner, state, theme),
        SubstrateOverlayTab::Assumptions => {
            assumptions_view::render_body(frame, inner, state, theme)
        }
    }
}

/// Mouse hit-test for the tab bar. Returns the Action to dispatch if a tab
/// label was clicked. Clicking the already-active tab closes the overlay;
/// clicking another tab cycles (single-step — matches the GateMonitor
/// pattern).
pub fn title_button_hit(col_in_rect: u16, state: &AppState) -> Option<Action> {
    let col = col_in_rect.saturating_sub(1); // account for rounded border
    let mut offset: u16 = TITLE_PREFIX.len() as u16;
    for (tab, label) in TAB_LABELS {
        let start = offset;
        let end = offset + (label.len() as u16);
        if (start..end).contains(&col) {
            if *tab == state.substrate_overlay_tab {
                return Some(Action::HideSubstrate);
            }
            return Some(Action::SubstrateOverlayToggleTab);
        }
        offset = end;
    }
    None
}
