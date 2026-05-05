//! Multipane run loop split across four submodules: [`event_loop`]
//! drives the loop, [`render`] paints, [`keys`] dispatches keyboard
//! events, [`mouse`] dispatches pointer events. The `pub(in
//! crate::multipane)` re-exports below are the only items reachable
//! from sibling multipane modules (`dispatch_focused`, `scroll`).

mod event_loop;
mod keys;
mod mouse;
mod render;

pub(in crate::multipane) use event_loop::capture_pane_mission_ids;
pub use event_loop::run_loop;
pub(in crate::multipane) use keys::{
    clear_focused_pane_input, focused_pane_agent_id, focused_pane_idx, focused_pane_mut,
    push_pane_system_message,
};
pub(in crate::multipane) use render::pane_body_rect;
#[cfg(test)]
#[allow(unused_imports)]
pub(in crate::multipane) use render::render_grid;

#[cfg(test)]
#[allow(unused_imports)]
use {
    crate::app::AbortScope,
    crate::multipane::dir_search_runner::DirSearchEvent,
    crate::multipane::scroll::scroll_chat_thread,
    crate::multipane::{grid, roster_view},
    crate::swarm::{SwarmRuntime, SYSTEM_ALERT_KIND},
    crate::vitals::VitalsState,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    event_loop::apply_dir_search_event,
    keys::{
        abort_focused_pane, close_focused_dir_search, collapse_at_cursor,
        collapse_dir_search_at_cursor, commit_dir_search, expand_at_cursor,
        expand_dir_search_at_cursor, focused_pane_dir_search_active, focused_pane_in_roster_mode,
        jump_roster_cursor_to_bottom, jump_roster_cursor_to_top, move_roster_cursor,
        move_selected_down, move_selected_up, revert_focused_pane_to_roster,
        with_focused_dir_search,
    },
    mouse::{
        apply_roster_click, handle_mouse, handle_mouse_scroll, resolve_left_click_target,
        try_open_chat_pane_artifact, RosterClickTarget,
    },
    nit_core::AppState,
    ratatui::layout::Rect,
    ratatui::style::{Modifier, Style},
    render::{compute_dropdown_rows, dir_search_body_rect, paint_bar, pane_inner_after_chrome},
};

#[cfg(test)]
#[path = "../../tests/multipane_runtime.rs"]
mod tests;
