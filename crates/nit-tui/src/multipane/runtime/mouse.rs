use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nit_core::{AppState, UiSelection, UiSelectionPane};
use ratatui::layout::Rect;

use super::keys::push_pane_system_message;
use super::render::{pane_at, pane_at_mut, pane_body_rect, pane_inner_after_chrome};
use crate::multipane::dispatch::with_pane_aliased;
use crate::multipane::focus;
use crate::multipane::grid;
use crate::multipane::roster_view;
use crate::multipane::scroll::{
    focused_pane_chat_thread_max_scroll, pane_thread_area_for_pane, resolve_chat_scroll_sentinel,
};
use crate::multipane::selection;
use crate::multipane::setup::materialise_pane_lane;
use crate::swarm::SwarmRuntime;
use crate::theme::Theme;
use crate::widgets::agent_console_view;
use crate::widgets::artifacts_popup;

pub(super) fn point_in_rect(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

pub(super) fn handle_mouse(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
    mouse: MouseEvent,
) {
    // The renderer reserves the top status strip and bottom hint row
    // before painting panes (see `render_grid`). Click hit-tests must
    // strip the same chrome — without this, `pane_at_point` returns
    // the pane one row above the cursor, so clicking
    // `Backend(Claude)` lands on `Backend(Codex)` etc.
    let grid_area = if area.height >= 4 {
        Rect::new(area.x, area.y + 1, area.width, area.height - 2)
    } else {
        area
    };
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            handle_mouse_left_down(state, swarm, theme, clipboard, grid_area, mouse);
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            handle_mouse_left_drag(state, swarm, theme, clipboard, grid_area, mouse);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            handle_mouse_left_up(state, swarm, grid_area, mouse.column, mouse.row);
        }
        MouseEventKind::ScrollUp => {
            handle_mouse_scroll(state, swarm, grid_area, mouse.column, mouse.row, -3);
        }
        MouseEventKind::ScrollDown => {
            handle_mouse_scroll(state, swarm, grid_area, mouse.column, mouse.row, 3);
        }
        _ => {}
    }
}

// Per-thread anchor for an in-progress popup body selection. Mirrors
// `InputState::mouse_select_anchor` from the single-pane handler.
// Stored here rather than on `AppState` because the anchor's lifetime
// is bounded by a single mouse-down → drag → up gesture, so leaking
// it across multipane / single-pane boundaries would just be noise.
thread_local! {
    static POPUP_BODY_ANCHOR: std::cell::Cell<Option<(usize, usize)>>
        = const { std::cell::Cell::new(None) };
    /// Per-gesture sentinel: which pane currently owns an in-progress
    /// chat-input-box drag. Mirrors single-pane's
    /// `MouseSelectTarget::ChatInput` flag — when Some, drag events
    /// extend the input-box selection on that pane rather than the
    /// chat-thread selection.
    static INPUT_BOX_DRAG_PANE: std::cell::Cell<Option<usize>>
        = const { std::cell::Cell::new(None) };
}

fn record_popup_anchor(line: usize, col: usize) {
    POPUP_BODY_ANCHOR.with(|cell| cell.set(Some((line, col))));
}

fn read_popup_anchor() -> Option<(usize, usize)> {
    POPUP_BODY_ANCHOR.with(|cell| cell.get())
}

fn clear_popup_anchor() {
    POPUP_BODY_ANCHOR.with(|cell| cell.set(None));
}

/// Lightweight equivalent of
/// `app::ui_selection::update_ui_selection_text` without the
/// `InputState`-backed dedup. Multipane's popup selection gesture is
/// bounded (down → drag → up), and writing the same text twice in a
/// row to the clipboard is harmless, so we skip the signature cache
/// and copy on every selection change.
fn copy_popup_selection_to_clipboard(
    state: &mut AppState,
    lines: &[String],
    clipboard: &mut Option<arboard::Clipboard>,
) {
    let Some(selection) = state.ui_selection else {
        return;
    };
    if selection.pane != UiSelectionPane::ArtifactsPopup {
        return;
    }
    let text = crate::app::selection_text(lines, selection);
    if text.is_empty() {
        return;
    }
    state.yank = Some(text.clone());
    state.yank_kind = if text.contains('\n') {
        nit_core::YankKind::Line
    } else {
        nit_core::YankKind::Char
    };
    if let Some(cb) = clipboard.as_mut() {
        let _ = cb.set_text(text);
    }
}

fn handle_mouse_left_down(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
    mouse: MouseEvent,
) {
    let x = mouse.column;
    let y = mouse.row;
    if state.agents.artifacts_popup_open {
        // Mirror the single-pane handler at `app/mouse.rs:779-836`:
        //   1. Click inside popup body → seed a text-selection anchor
        //      and start a single-point UiSelection. Drag extends it.
        //   2. Click on the popup chat input → cursor positioning +
        //      input-buffer selection anchor.
        //   3. Click outside the popup → close popup.
        // Multipane stores the body anchor in a thread_local
        // (`POPUP_BODY_ANCHOR`) instead of `InputState`, but the
        // selection state lives on `AppState.ui_selection` like the
        // single-pane case, so the renderer highlights identically.
        let popup_area =
            super::event_loop::popup_rect_for(area, artifacts_popup::preferred_size(area));

        // Chat input within the popup — cursor positioning +
        // selection anchor on the input buffer (matches single-pane
        // behaviour).
        if let Some(cursor_char_idx) =
            artifacts_popup::map_chat_input_point_to_cursor(state, swarm, popup_area, x, y, false)
        {
            let total_chars = state.agents.artifacts_popup_chat_input.chars().count();
            let new_cursor = cursor_char_idx.min(total_chars);
            let cursor_pos = state.agents.artifacts_popup_chat_cursor;
            set_anchor_if_extending(
                mouse.modifiers.contains(KeyModifiers::SHIFT),
                new_cursor,
                cursor_pos,
                total_chars,
                &mut state.agents.artifacts_popup_chat_selection_anchor,
            );
            state.agents.artifacts_popup_chat_cursor = new_cursor;
            // Body anchor reset — clicking the input clears any
            // pending body drag.
            clear_popup_anchor();
            // The popup chat input has its own selection-text path
            // separate from `ui_selection`; leave that to keypath
            // handlers for now (out of scope this round).
            return;
        }

        // Click on a body line → start a body selection.
        if let Some((line_idx, col, lines)) = crate::app::map_artifacts_popup_mouse_with_swarm(
            swarm, mouse, area, state, theme, false,
        ) {
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::ArtifactsPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            record_popup_anchor(line_idx, col);
            copy_popup_selection_to_clipboard(state, &lines, clipboard);
            return;
        }

        // Inside popup but neither chat-input nor body — likely a
        // border / padding click. No-op (don't close).
        if point_in_rect(x, y, popup_area) {
            return;
        }
        // Outside the popup → close.
        state.agents.artifacts_popup_open = false;
        clear_popup_anchor();
        if matches!(state.ui_selection, Some(s) if s.pane == UiSelectionPane::ArtifactsPopup) {
            state.ui_selection = None;
        }
        return;
    }
    // Input-box click: position cursor + seed input selection anchor.
    // Mirrors single-pane behaviour at `app/mouse.rs:1127-1148`.
    // Tested BEFORE the chat-thread hit-test so an input-box click
    // never spills into the thread-selection branch.
    if let Some((pane_idx, cursor_char_idx)) = resolve_pane_input_box_hit(state, area, x, y) {
        if let Some(mp) = state.multipane.as_mut() {
            mp.focused = pane_idx;
        }
        with_pane_aliased(state, pane_idx, |state| {
            let total_chars = state.agents.chat_input.chars().count();
            let new_cursor = cursor_char_idx.min(total_chars);
            let cursor_pos = state.agents.chat_input_cursor;
            set_anchor_if_extending(
                mouse.modifiers.contains(KeyModifiers::SHIFT),
                new_cursor,
                cursor_pos,
                total_chars,
                &mut state.agents.chat_input_selection_anchor,
            );
            state.agents.chat_input_cursor = new_cursor;
            // Mirror single-pane (`app/mouse.rs:1153`): every
            // selection mutation auto-copies, so a shift-click that
            // grew the selection is immediately on the clipboard
            // without the operator having to also press Cmd+C.
            crate::app::copy_chat_input_selection(state, clipboard);
        });
        INPUT_BOX_DRAG_PANE.with(|cell| cell.set(Some(pane_idx)));
        return;
    }
    // Drag-to-select takes precedence over artifact-popup open: seed
    // the selection anchor on Down, defer popup-open to Up so a
    // single click without drag still opens the popup. Selection
    // lives on the pane that owns the click — never the focused pane
    // — so dragging inside an unfocused pane creates a per-pane
    // selection.
    //
    // The *swarm-aware* resolver is critical here: when the pane's
    // `chat_thread_scroll == CONSOLE_SCROLL_BOTTOM` (the "follow
    // bottom" sentinel — true for any pane that hasn't been scrolled
    // by hand), the no-swarm variant treats it as `0` and the
    // selection lands on the wrong row. Passing the swarm runtime
    // resolves the sentinel to the actual `max_scroll` so the row
    // index lines up with what the renderer painted.
    if let Some((pane_idx, line_idx, col_idx)) =
        resolve_chat_thread_hit_with_swarm(state, Some(swarm), area, x, y)
    {
        let Some(pane) = pane_at_mut(state, pane_idx) else {
            return;
        };
        selection::clear(pane);
        selection::extend_to(pane, line_idx, col_idx);
        return;
    }
    let Some(target) = resolve_left_click_target(state, area, x, y) else {
        return;
    };
    apply_roster_click(state, target);
}

/// Resolve a screen `(x, y)` to `(pane_idx, char_index_into_chat_input)`
/// when the click lands inside any pane's chat input box.
fn resolve_pane_input_box_hit(
    state: &AppState,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<(usize, usize)> {
    let mp = state.multipane.as_ref()?;
    let pane_idx = grid::pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y)?;
    let pane = mp.panes.get(pane_idx)?;
    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        return None;
    }
    let pane_rect = grid::pane_rect(area, mp.grid_cols, mp.grid_rows, pane_idx);
    let inner = pane_inner_after_chrome(pane_rect);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let cursor_char_idx =
        agent_console_view::map_pane_chat_input_point_to_cursor(inner, pane, x, y, false)?;
    Some((pane_idx, cursor_char_idx))
}

fn handle_mouse_left_drag(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
    mouse: MouseEvent,
) {
    let x = mouse.column;
    let y = mouse.row;

    // Popup body drag: extend the anchor → current point UiSelection
    // and re-copy to clipboard. Mirrors the single-pane drag handler
    // (`app/mouse.rs::handle_mouse_drag_with_swarm` UiSelectionPane
    // branch) but reads the anchor from the multipane thread_local
    // since `InputState` isn't plumbed here.
    if state.agents.artifacts_popup_open {
        if let Some((anchor_line, anchor_col)) = read_popup_anchor() {
            // Match the single-pane drag: bump scroll when the cursor
            // leaves the visible body so the operator can extend the
            // selection past the rendered window.
            crate::app::auto_scroll_artifacts_popup_for_drag(swarm, mouse, area, state);
            if let Some((line_idx, col, lines)) = crate::app::map_artifacts_popup_mouse_with_swarm(
                swarm, mouse, area, state, theme, true,
            ) {
                state.ui_selection = Some(UiSelection {
                    pane: UiSelectionPane::ArtifactsPopup,
                    start_line: anchor_line,
                    start_col: anchor_col,
                    end_line: line_idx,
                    end_col: col,
                });
                copy_popup_selection_to_clipboard(state, &lines, clipboard);
            }
        }
        return;
    }

    // Input-box drag: extend the chat-input selection. The
    // thread_local sentinel (not the chat-thread hit-test) keeps the
    // drag targeted at the input box even when the cursor moves
    // outside its rect; clamping is enabled so the selection expands
    // to the row/col edge.
    if let Some(pane_idx) = INPUT_BOX_DRAG_PANE.with(|cell| cell.get()) {
        let pane = match state
            .multipane
            .as_ref()
            .and_then(|mp| mp.panes.get(pane_idx))
        {
            Some(pane) => pane.clone(),
            None => {
                INPUT_BOX_DRAG_PANE.with(|cell| cell.set(None));
                return;
            }
        };
        let mp_grid = state
            .multipane
            .as_ref()
            .map(|mp| (mp.grid_cols, mp.grid_rows));
        let Some((grid_cols, grid_rows)) = mp_grid else {
            return;
        };
        let pane_rect = grid::pane_rect(area, grid_cols, grid_rows, pane_idx);
        let inner = pane_inner_after_chrome(pane_rect);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if let Some(cursor_char_idx) =
            agent_console_view::map_pane_chat_input_point_to_cursor(inner, &pane, x, y, true)
        {
            with_pane_aliased(state, pane_idx, |state| {
                let total_chars = state.agents.chat_input.chars().count();
                let new_cursor = cursor_char_idx.min(total_chars);
                if state.agents.chat_input_selection_anchor.is_none() {
                    state.agents.chat_input_selection_anchor =
                        Some(state.agents.chat_input_cursor.min(total_chars));
                }
                state.agents.chat_input_cursor = new_cursor;
                // Match single-pane drag behaviour
                // (`app/mouse.rs:1641`) — auto-copy on every drag tick
                // so releasing the mouse leaves the selection already
                // on the clipboard.
                crate::app::copy_chat_input_selection(state, clipboard);
            });
        }
        return;
    }

    // Auto-scroll the pane that owns the active selection when the
    // drag cursor leaves its thread rect through the top or bottom.
    // Has to run BEFORE `resolve_chat_thread_hit_with_swarm` (which
    // returns None on out-of-rect mouse positions) so the cursor
    // moving below the bottom edge keeps extending the selection
    // instead of stalling.
    auto_scroll_drag_pane_chat_thread(state, swarm, area, mouse.row);

    // Same sentinel concern as the Down handler: a drag whose start
    // point landed on a sentinel-scrolled pane needs the swarm-aware
    // resolver, otherwise `chat_thread_scroll` is treated as 0 and
    // the selection extends to a row that has nothing to do with
    // what's visually under the cursor.
    let Some((pane_idx, line_idx, col_idx)) =
        resolve_chat_thread_hit_with_swarm(state, Some(swarm), area, x, y)
    else {
        return;
    };
    let owns_anchor = pane_at(state, pane_idx)
        .and_then(|p| p.selection.as_ref().map(|_| ()))
        .is_some();
    if !owns_anchor {
        return;
    }
    if let Some(pane) = pane_at_mut(state, pane_idx) {
        selection::extend_to(pane, line_idx, col_idx);
    }
}

/// Finds the pane currently owning a chat-thread selection (only one
/// pane can be mid-drag at a time) and bumps its `chat_thread_scroll`
/// when `mouse_y` is outside its rendered thread rect. Mirrors
/// `app::auto_scroll_agent_console_for_drag` but for per-pane scroll
/// state.
fn auto_scroll_drag_pane_chat_thread(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    area: Rect,
    mouse_y: u16,
) {
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let Some(pane_idx) = mp.panes.iter().position(|p| p.selection.is_some()) else {
        return;
    };
    let pane = match mp.panes.get(pane_idx) {
        Some(p) => p.clone(),
        None => return,
    };
    let Some(thread_area) = pane_thread_area_for_pane(state, area, pane_idx, &pane) else {
        return;
    };
    let max_scroll = focused_pane_chat_thread_max_scroll(state, swarm, &pane, area, pane_idx);
    let mut scroll = resolve_chat_scroll_sentinel(pane.chat_thread_scroll, max_scroll);
    if crate::app::drag_auto_scroll(
        mouse_y,
        thread_area.y,
        thread_area.height,
        &mut scroll,
        max_scroll,
    ) {
        if let Some(p) = pane_at_mut(state, pane_idx) {
            p.chat_thread_scroll = scroll;
        }
    }
}

fn handle_mouse_left_up(state: &mut AppState, swarm: &SwarmRuntime, area: Rect, x: u16, y: u16) {
    // Popup body anchor lives only for the duration of the gesture.
    // Drop it on Up regardless of pane hit so a stray release outside
    // the popup doesn't leak the anchor across gestures.
    if state.agents.artifacts_popup_open {
        clear_popup_anchor();
        return;
    }
    // Input-box drag terminator: drop the per-gesture sentinel so the
    // next mouse-down starts a fresh selection. The pane's
    // chat_input_selection_anchor stays set if the drag covered any
    // characters — Cmd+C / Ctrl+C on the canonical handler picks it
    // up.
    if INPUT_BOX_DRAG_PANE
        .with(|cell| cell.replace(None))
        .is_some()
    {
        return;
    }
    let Some((pane_idx, _, _)) = resolve_chat_thread_hit_with_swarm(state, Some(swarm), area, x, y)
    else {
        return;
    };
    let collapsed = pane_at(state, pane_idx)
        .and_then(|p| p.selection.as_ref())
        .map(|s| (s.anchor_line, s.anchor_col) == (s.end_line, s.end_col))
        .unwrap_or(true);
    if !collapsed {
        // Real drag: keep the selection — Ctrl/Cmd+C copies it later.
        return;
    }
    if let Some(pane) = pane_at_mut(state, pane_idx) {
        selection::clear(pane);
    }
    if try_open_chat_pane_artifact(state, swarm, area, x, y) {
        return;
    }
    if let Some(target) = resolve_left_click_target(state, area, x, y) {
        apply_roster_click(state, target);
    }
}

/// Resolve a screen `(x, y)` to `(pane_idx, logical_line, char_col)`
/// inside that pane's chat thread. `logical_line` already includes
/// `chat_thread_scroll`, so it's directly usable as a row index into
/// `build_pane_thread_rows`. The `swarm` argument is required when
/// `chat_thread_scroll` may hold the `CONSOLE_SCROLL_BOTTOM` sentinel
/// (true for any pane that hasn't been scrolled by hand) — without
/// it, sentinel resolution falls back to 0 and the resolved line is
/// wrong by `max_scroll` rows. Pass `None` only when the caller is
/// certain the sentinel never applies (tests that pre-set scroll to
/// a numeric value).
fn resolve_chat_thread_hit_with_swarm(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<(usize, usize, usize)> {
    let mp = state.multipane.as_ref()?;
    let pane_idx = grid::pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y)?;
    let pane = mp.panes.get(pane_idx)?;
    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        return None;
    }
    let thread_area = pane_thread_area_for_pane(state, area, pane_idx, pane)?;
    if !point_in_rect(x, y, thread_area) {
        return None;
    }
    let local_y = (y - thread_area.y) as usize;
    let local_x = (x - thread_area.x) as usize;
    // Resolve the sentinel "follow bottom" to a concrete row offset.
    // Falls back to a clamp against the renderer's max_scroll when a
    // swarm runtime is available; without one, treat the sentinel as
    // 0 so the click maps somewhere reasonable rather than
    // overflowing.
    let scroll = if pane.chat_thread_scroll == nit_core::CONSOLE_SCROLL_BOTTOM {
        match swarm {
            Some(s) => focused_pane_chat_thread_max_scroll(state, s, pane, area, pane_idx),
            None => 0,
        }
    } else {
        pane.chat_thread_scroll
    };
    Some((pane_idx, scroll.saturating_add(local_y), local_x))
}

/// If `(x, y)` lands on a chat-pane thread row that the artifact-popup
/// resolver recognises, open the popup and return `true`. Otherwise
/// returns `false` so the caller can fall through to roster click
/// resolution.
pub(super) fn try_open_chat_pane_artifact(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    area: Rect,
    x: u16,
    y: u16,
) -> bool {
    let Some((pane_idx, line_idx, _col)) =
        resolve_chat_thread_hit_with_swarm(state, Some(swarm), area, x, y)
    else {
        return false;
    };
    let Some(pane) = pane_at(state, pane_idx).cloned() else {
        return false;
    };
    let Some(thread_area) = pane_thread_area_for_pane(state, area, pane_idx, &pane) else {
        return false;
    };
    invoke_pane_artifact_popup(
        state,
        swarm,
        pane_idx,
        &pane,
        thread_area.width as usize,
        line_idx,
    )
}

/// On a successful open `popup_keys` deliberately writes
/// `selected_mission` to bind the popup to the clicked artifact —
/// leave those values in place. Restore on miss so other panes don't
/// see contaminated globals.
fn invoke_pane_artifact_popup(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    pane_idx: usize,
    pane: &nit_core::PaneSession,
    text_width: usize,
    line_idx: usize,
) -> bool {
    let saved_agent = state.agents.selected_agent.clone();
    let saved_mission = state.agents.selected_mission.clone();
    // Stamp the mission_selected sentinel for parity with
    // with_pane_aliased — without this,
    // selected_context_mission()'s missions[mission_selected]
    // fallback can return another pane's mission and the artifact
    // popup resolver walks the wrong thread.
    let saved_mission_selected = state.agents.mission_selected;
    let pane_agent_id = if pane.agent_id.is_empty() {
        pane.selected_agent_id.clone()
    } else {
        Some(pane.agent_id.clone())
    };
    state.agents.selected_agent = pane_agent_id;
    state.agents.selected_mission = pane.mission_id.clone();
    state.agents.mission_selected = usize::MAX;
    // Pane-aware variant: the resolver must walk the same pane-scoped
    // message list the renderer used (`message_matches_pane`),
    // otherwise an inline breather (e.g. active shadow run) shifts
    // the row cursor and clicking `(see ARTIFACTS)` misses entirely.
    let opened = crate::app::popup_keys::maybe_open_artifact_popup_from_console_line_for_pane(
        state,
        Some(swarm),
        Some(pane_idx),
        text_width,
        line_idx,
    );
    if !opened {
        state.agents.selected_agent = saved_agent;
        state.agents.selected_mission = saved_mission;
        state.agents.mission_selected = saved_mission_selected;
    }
    opened
}

pub(super) struct RosterClickTarget {
    pub(super) pane_idx: usize,
    pub(super) rows: Vec<roster_view::PaneRosterRow>,
    pub(super) row_idx: usize,
    pub(super) row: roster_view::PaneRosterRow,
    pub(super) local_x: usize,
}

pub(super) fn resolve_left_click_target(
    state: &mut AppState,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<RosterClickTarget> {
    let mp = state.multipane.as_mut()?;
    let pane_idx = focus::focus_at_point(mp, area, x, y)?;
    let backend_filter = mp.backend_filter.clone();
    let pane = mp.panes.get(pane_idx).cloned()?;
    if !(pane.selected_agent_id.is_none() && pane.agent_id.is_empty()) {
        return None; // chat panes ignore left-clicks beyond focus
    }
    let body = pane_body_rect(state, area, pane_idx, &pane)?;
    if !point_in_rect(x, y, body) {
        return None;
    }
    let local_x = (x - body.x) as usize;
    let local_y = (y - body.y) as usize;
    let rows = roster_view::compute_rows(state, &pane, backend_filter.as_deref());
    let row_idx = roster_view::row_index_at_y(&rows, pane.roster_scroll, local_y)?;
    let row = rows.get(row_idx).cloned()?;
    Some(RosterClickTarget {
        pane_idx,
        rows,
        row_idx,
        row,
        local_x,
    })
}

pub(super) fn apply_roster_click(state: &mut AppState, target: RosterClickTarget) {
    let RosterClickTarget {
        pane_idx,
        rows,
        row_idx,
        row,
        local_x,
    } = target;
    match row {
        roster_view::PaneRosterRow::Template => {
            if let Some(value) = roster_view::template_word_at_x(local_x) {
                if let Some(pane) = pane_at_mut(state, pane_idx) {
                    pane.swarm_template = value.into();
                }
            }
        }
        roster_view::PaneRosterRow::Mission => {
            if let Some(value) = roster_view::mission_word_at_x(local_x) {
                if let Some(pane) = pane_at_mut(state, pane_idx) {
                    pane.swarm_mission = value.into();
                }
            }
        }
        roster_view::PaneRosterRow::Backend { .. } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            commit_agent_to_pane(state, pane_idx, &agent_id);
        }
        roster_view::PaneRosterRow::SizeBranch { agent_id } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            if let Some(pane) = pane_at_mut(state, pane_idx) {
                roster_view::toggle_agent_tree_collapse(pane, &agent_id);
            }
        }
        roster_view::PaneRosterRow::SizeLeaf {
            agent_id, leaf_idx, ..
        } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            roster_view::toggle_size_leaf(state, pane_idx, &agent_id, leaf_idx);
        }
        roster_view::PaneRosterRow::Empty(_) | roster_view::PaneRosterRow::Spacer => {}
    }
}

fn seek_pane_cursor_to(
    state: &mut AppState,
    pane_idx: usize,
    rows: &[roster_view::PaneRosterRow],
    row_idx: usize,
) {
    let Some(cursor) = roster_view::cursor_for_row_index(rows, row_idx) else {
        return;
    };
    let row = rows.get(row_idx).cloned();
    if let Some(pane) = pane_at_mut(state, pane_idx) {
        pane.roster_cursor = cursor;
        roster_view::sync_tree_selection(pane, row.as_ref());
        roster_view::sync_auto_expansion(pane, row.as_ref());
    }
}

fn commit_agent_to_pane(state: &mut AppState, pane_idx: usize, agent_id: &str) {
    let message = match materialise_pane_lane(state, pane_idx, agent_id) {
        Some(id) => format!("selected agent → {id}"),
        None => format!("could not materialise pane lane for {agent_id}"),
    };
    push_pane_system_message(state, message);
}

pub(super) fn handle_mouse_scroll(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    area: Rect,
    x: u16,
    y: u16,
    delta: i32,
) {
    // Modal: wheel events while the artifacts popup is open scroll
    // the popup, not the chat thread underneath. Match
    // `app/mouse.rs`.
    if state.agents.artifacts_popup_open {
        let popup_area =
            super::event_loop::popup_rect_for(area, artifacts_popup::preferred_size(area));
        if point_in_rect(x, y, popup_area) {
            let max_scroll = state.agents.artifacts_popup_last_max_scroll;
            // The renderer re-clamps each frame, so a stale scroll
            // value self-corrects on next draw — safe to advance
            // optimistically.
            let current = state.agents.artifacts_popup_scroll as i32;
            let next = (current + delta).max(0) as usize;
            state.agents.artifacts_popup_scroll = next.min(max_scroll.max(0));
        }
        return;
    }
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let Some(pane_idx) = grid::pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y) else {
        return;
    };
    let Some(pane) = mp.panes.get(pane_idx).cloned() else {
        return;
    };
    let in_roster = pane.selected_agent_id.is_none() && pane.agent_id.is_empty();
    let max_scroll = if in_roster {
        roster_max_scroll(state, &pane, area, pane_idx)
    } else {
        focused_pane_chat_thread_max_scroll(state, swarm, &pane, area, pane_idx)
    };
    let Some(p) = pane_at_mut(state, pane_idx) else {
        return;
    };
    if in_roster {
        let current = p.roster_scroll as i32;
        let next = (current + delta).max(0) as usize;
        p.roster_scroll = next.min(max_scroll);
    } else {
        // Wheel uses the same sentinel-resolution path as the
        // keyboard scroll — see `resolve_chat_scroll_sentinel` and
        // `scroll_chat_thread` for the matching delta-guard
        // rationale.
        let resolved = resolve_chat_scroll_sentinel(p.chat_thread_scroll, max_scroll);
        let next = (resolved as i32 + delta).max(0) as usize;
        p.chat_thread_scroll = if delta > 0 && next >= max_scroll {
            nit_core::CONSOLE_SCROLL_BOTTOM
        } else {
            next.min(max_scroll)
        };
    }
}

fn roster_max_scroll(
    state: &AppState,
    pane: &nit_core::PaneSession,
    area: Rect,
    pane_idx: usize,
) -> usize {
    let height = pane_body_rect(state, area, pane_idx, pane)
        .map(|body| body.height as usize)
        .unwrap_or(0);
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let rows = roster_view::compute_rows(state, pane, backend_filter.as_deref());
    rows.len().saturating_sub(height)
}

/// Returns `true` when a non-empty selection was copied (and cleared);
/// `false` otherwise so the caller can fall through to the abort
/// path. Width is computed from the focused pane's render area so
/// wrap boundaries match what the operator saw at drag time.
pub(super) fn try_copy_focused_pane_selection(
    state: &mut AppState,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
) -> bool {
    let Some(mp) = state.multipane.as_ref() else {
        return false;
    };
    let pane_idx = mp.focused;
    let Some(pane) = mp.panes.get(pane_idx).cloned() else {
        return false;
    };
    if pane.selection.is_none() {
        return false;
    }
    let in_chat_mode = pane.selected_agent_id.is_some() || !pane.agent_id.is_empty();
    if !in_chat_mode {
        return false;
    }
    let pane_rect = grid::pane_rect(area, mp.grid_cols, mp.grid_rows, pane_idx);
    let inner = pane_inner_after_chrome(pane_rect);
    let Some(thread_area) = agent_console_view::pane_thread_text_area(inner, &pane) else {
        return false;
    };
    let width = thread_area.width.max(1) as usize;
    let rows = agent_console_view::build_pane_thread_rows_for_pane(
        state,
        None,
        Some(pane.pane_id),
        Some(pane.agent_id.as_str()),
        pane.mission_id.as_deref().or_else(|| {
            (!pane.chat_mission_id.is_empty()).then_some(pane.chat_mission_id.as_str())
        }),
        width,
        !pane.has_run_mission,
    );
    let text = selection::resolve_text(&pane, &rows);
    let Some(text) = text else {
        if let Some(p) = pane_at_mut(state, pane_idx) {
            selection::clear(p);
        }
        return false;
    };
    if let Some(cb) = clipboard.as_mut() {
        let _ = cb.set_text(text);
    }
    if let Some(p) = pane_at_mut(state, pane_idx) {
        selection::clear(p);
    }
    true
}

pub(super) fn dispatch_commit(
    state: &mut AppState,
    pane_idx: usize,
    row: roster_view::PaneRosterRow,
) {
    match row {
        roster_view::PaneRosterRow::Backend { .. } => {
            // Enter on a Backend row drills the cursor down into the
            // group's first child, mirroring `l` / Right.
            if pane_idx == super::keys::focused_pane_idx(state) {
                super::keys::move_roster_cursor(state, 1);
            }
        }
        roster_view::PaneRosterRow::SizeBranch { agent_id } => {
            let Some(pane) = pane_at_mut(state, pane_idx) else {
                return;
            };
            roster_view::toggle_agent_tree_collapse(pane, &agent_id);
        }
        roster_view::PaneRosterRow::SizeLeaf {
            agent_id, leaf_idx, ..
        } => {
            roster_view::toggle_size_leaf(state, pane_idx, &agent_id, leaf_idx);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            commit_agent_to_pane(state, pane_idx, &agent_id);
        }
        roster_view::PaneRosterRow::Template
        | roster_view::PaneRosterRow::Mission
        | roster_view::PaneRosterRow::Empty(_)
        | roster_view::PaneRosterRow::Spacer => {}
    }
}

fn set_anchor_if_extending(
    shift: bool,
    new_cursor: usize,
    cursor_pos: usize,
    total_chars: usize,
    anchor: &mut Option<usize>,
) {
    if shift {
        if anchor.is_none() {
            *anchor = Some(cursor_pos.min(total_chars));
        }
    } else {
        *anchor = Some(new_cursor);
    }
}
