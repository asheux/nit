use nit_core::MultipaneState;
use ratatui::layout::Rect;

use super::grid::pane_at_point;
use crate::app::clear_chat_esc_state;

pub fn cycle_forward(mp: &mut MultipaneState) {
    if mp.panes.is_empty() {
        return;
    }
    mp.focused = (mp.focused + 1) % mp.panes.len();
    // Reset the Esc-Esc latch so an Esc pressed in the previous pane
    // can't combine with an Esc pressed in the new pane to abort the
    // wrong mission.
    clear_chat_esc_state();
}

pub fn cycle_backward(mp: &mut MultipaneState) {
    if mp.panes.is_empty() {
        return;
    }
    if mp.focused == 0 {
        mp.focused = mp.panes.len() - 1;
    } else {
        mp.focused -= 1;
    }
    clear_chat_esc_state();
}

pub fn focus_at_point(mp: &mut MultipaneState, area: Rect, x: u16, y: u16) -> Option<usize> {
    let idx = pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y)?;
    if idx >= mp.panes.len() {
        return None;
    }
    let prev = mp.focused;
    mp.focused = idx;
    if prev != idx {
        clear_chat_esc_state();
    }
    Some(idx)
}

#[cfg(test)]
#[path = "../tests/multipane_focus.rs"]
mod tests;
