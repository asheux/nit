//! Chat-thread scroll math split out of `multipane::runtime`.
//!
//! All entry points are pane-scoped (focused pane or an explicit pane
//! index): the renderer's max-scroll clamp at
//! `agent_console_view::render_pane` is mirrored here so PgUp / PgDn /
//! wheel never pin the stored scroll past the rendered window. The
//! "stick to bottom" sentinel (`CONSOLE_SCROLL_BOTTOM`) is resolved at
//! every read so a transient `max_scroll` dip (breather rows
//! oscillating mid-swarm) doesn't silently consume the operator's
//! scroll-up.

use nit_core::AppState;
use ratatui::layout::Rect;

use super::runtime::{focused_pane_mut, pane_body_rect};
use crate::swarm::SwarmRuntime;
use crate::widgets::agent_console_view;

pub(super) const CHAT_THREAD_PAGE_STEP: i32 = 8;

pub(super) fn scroll_chat_thread(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    area: Rect,
    delta: i32,
) {
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let focused_idx = mp.focused;
    let Some(pane) = mp.panes.get(focused_idx).cloned() else {
        return;
    };
    let max_scroll = focused_pane_chat_thread_max_scroll(state, swarm, &pane, area, focused_idx);
    if let Some(p) = focused_pane_mut(state) {
        // Resolve the "stick to bottom" sentinel before applying delta —
        // otherwise PgUp from the bottom jumps to row 0 instead of one
        // page above the bottom (sentinel `as i32` wraps to -1 and
        // `(-1 + delta).max(0) = 0`).
        let resolved = resolve_chat_scroll_sentinel(p.chat_thread_scroll, max_scroll);
        let next = (resolved as i32 + delta).max(0) as usize;
        // Only re-engage the "stick to bottom" sentinel when the operator
        // scrolled DOWN past the current bottom. PgUp / wheel-up must
        // never re-engage it — otherwise a transient max_scroll dip
        // (breather rows oscillating mid-swarm) silently consumes the
        // operator's scroll-up and the viewport feels stuck.
        p.chat_thread_scroll = if delta > 0 && next >= max_scroll {
            nit_core::CONSOLE_SCROLL_BOTTOM
        } else {
            next.min(max_scroll)
        };
    }
}

/// Translate the "follow bottom" sentinel into a concrete row offset
/// for arithmetic. Other values pass through. Used by both the
/// keyboard PgUp/PgDn path and the mouse wheel path so they stay in
/// lockstep with each other and with the `min(max_scroll)` clamp the
/// renderer applies.
pub(super) fn resolve_chat_scroll_sentinel(scroll: usize, max_scroll: usize) -> usize {
    if scroll == nit_core::CONSOLE_SCROLL_BOTTOM {
        max_scroll
    } else {
        scroll.min(max_scroll)
    }
}

/// Maximum legal `chat_thread_scroll` for the given pane. Mirrors the
/// renderer's clamp at `agent_console_view::render_pane` so wheel /
/// PgUp / PgDn never pin the stored scroll beyond the rendered window
/// — which would otherwise force the operator to "drain" stale scroll
/// before any visible movement happens.
pub(super) fn focused_pane_chat_thread_max_scroll(
    state: &AppState,
    swarm: &SwarmRuntime,
    pane: &nit_core::PaneSession,
    area: Rect,
    pane_idx: usize,
) -> usize {
    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        return 0;
    }
    let Some(thread_area) = pane_thread_area_for_pane(state, area, pane_idx, pane) else {
        return 0;
    };
    let agent_id = if pane.agent_id.is_empty() {
        pane.selected_agent_id.as_deref()
    } else {
        Some(pane.agent_id.as_str())
    };
    let rows = agent_console_view::build_pane_thread_rows_with_breathers_for_pane(
        state,
        Some(swarm),
        Some(pane.pane_id),
        agent_id,
        pane.mission_id.as_deref().or_else(|| {
            (!pane.chat_mission_id.is_empty()).then_some(pane.chat_mission_id.as_str())
        }),
        thread_area.width.max(1) as usize,
        !pane.has_run_mission,
    );
    rows.len()
        .saturating_sub(thread_area.height.max(1) as usize)
}

pub(super) fn pane_thread_area_for_pane(
    state: &AppState,
    area: Rect,
    pane_idx: usize,
    pane: &nit_core::PaneSession,
) -> Option<Rect> {
    let body = pane_body_rect(state, area, pane_idx, pane)?;
    agent_console_view::pane_thread_text_area(body, pane)
}
