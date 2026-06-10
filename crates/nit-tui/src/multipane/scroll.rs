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

/// Both the keyboard PgUp/PgDn path and the mouse wheel path call this
/// so they stay in lockstep with each other and with the
/// `min(max_scroll)` clamp the renderer applies.
pub(super) fn resolve_chat_scroll_sentinel(scroll: usize, max_scroll: usize) -> usize {
    if scroll == nit_core::CONSOLE_SCROLL_BOTTOM {
        max_scroll
    } else {
        scroll.min(max_scroll)
    }
}

/// Returns the renderer-cached `max_scroll` for this pane's chat thread
/// (written every frame by `agent_console_view::render_pane`). Reading the
/// cached value guarantees the wheel / PgUp / PgDn clamp matches the renderer's
/// exactly. The previous implementation re-derived the bound from a
/// reconstructed thread area, which drifted from the renderer's real layout
/// (chrome + dir-search overlay + dynamic input height) and made scrolling jump
/// straight to the top or bottom. The extra params are kept so the call sites
/// (which already hold `state` / `swarm` / `area` / `pane_idx`) stay untouched.
pub(super) fn focused_pane_chat_thread_max_scroll(
    _state: &AppState,
    _swarm: &SwarmRuntime,
    pane: &nit_core::PaneSession,
    _area: Rect,
    _pane_idx: usize,
) -> usize {
    pane.chat_thread_last_max_scroll
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
