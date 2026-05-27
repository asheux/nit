//! State-layer jumplist helpers.
//!
//! The vim-style jumplist *data structure* lives next to the buffer module
//! at `buffer::jumplist` because the ring entries are buffer-local (line,
//! col, buffer_id). This module wraps that ring with the state-layer
//! concerns that only `AppState` can answer:
//!
//! * Which `buffer_id` is the focused editor *right now*?
//! * Are we about to jump cross-buffer (NITTree row activation, Ctrl-P
//!   picker, Ctrl-F content match) and where does the destination
//!   buffer live in `AppState::buffers`?
//! * Should we anchor the live cursor onto the ring before walking back,
//!   so `Ctrl-O` immediately followed by `Ctrl-I` round-trips even when
//!   the most-recent push hasn't been seen yet?
//!
//! Keeping the data structure in `buffer::jumplist` (single-buffer concern)
//! and the policy here (cross-buffer + apply-to-state concern) prevents the
//! buffer module from pulling in `AppState`, which would create a circular
//! module dependency.

use super::AppState;
use crate::buffer::{JumpEntry, JumpList};
use crate::cursor::Cursor;
use crate::mode::Mode;
use crate::pane::PaneId;

/// Outcome of a `Ctrl-O` / `Ctrl-I` step. Callers use it to decide whether
/// to switch buffers (cross-buffer), reposition the cursor (in-buffer), or
/// surface an empty-ring status message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JumpStepOutcome {
    /// The ring returned an entry inside the focused buffer; the caller
    /// should move the cursor to `(line, col)`.
    InBuffer { line: usize, col: usize },
    /// The ring returned an entry pointing at a different buffer than the
    /// focused one. `apply_step` resolves it to a buffer swap; the raw
    /// variant is kept so older call-sites (and tests) can still inspect
    /// the classification.
    CrossBuffer {
        target_buffer_id: usize,
        line: usize,
        col: usize,
    },
    /// The ring is empty in the requested direction, or every entry past
    /// the navigation cursor pointed at buffers that no longer exist.
    Empty,
}

/// Direction of a single jumplist step. Public so the action arm and the
/// chord-layer fast path can both reach the shared `apply_step` helper
/// instead of duplicating the back/forward branching.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum JumpDirection {
    Back,
    Forward,
}

/// Push `(buffer_id, cursor)` onto `list`. Mirrors `JumpList::push` for
/// callers that only have a `Cursor` handy.
pub fn push_cursor(list: &mut JumpList, buffer_id: usize, cursor: Cursor) {
    list.push(JumpEntry::new(buffer_id, cursor.line, cursor.col));
}

/// Walk one step back through the ring. Returns the raw classification so
/// older callers can still inspect it; for the canonical apply-to-state
/// path use [`apply_step`].
pub fn step_back(list: &mut JumpList, focused_buffer_id: usize) -> JumpStepOutcome {
    classify_step(list.jump_back(), focused_buffer_id)
}

/// Walk one step forward through the ring. See [`step_back`].
pub fn step_forward(list: &mut JumpList, focused_buffer_id: usize) -> JumpStepOutcome {
    classify_step(list.jump_forward(), focused_buffer_id)
}

/// Drive a `Ctrl-O` / `Ctrl-I` step against `AppState`: handles the
/// freshest-end anchor (so back+forward round-trips), skips ring entries
/// whose `buffer_id` no longer points at a live editor buffer, switches
/// `active_editor_buffer_id` on cross-buffer outcomes, and repositions
/// the destination cursor clamped to the line's visible width.
///
/// Returns the (post-validation) outcome so callers can drive their own
/// status messaging without re-classifying.
pub fn apply_step(state: &mut AppState, dir: JumpDirection) -> JumpStepOutcome {
    let focused_buffer_id = state.active_editor_buffer_id;
    let current = state.current_jump_entry();
    if matches!(dir, JumpDirection::Back) {
        anchor_current_at_freshest_end(state, current);
    }
    let outcome = walk_until_live(state, dir, focused_buffer_id);
    match outcome {
        JumpStepOutcome::Empty => {
            state.status = Some(empty_ring_message(dir).into());
        }
        JumpStepOutcome::InBuffer { line, col } => {
            place_focused_cursor(state, line, col);
        }
        JumpStepOutcome::CrossBuffer {
            target_buffer_id,
            line,
            col,
        } => {
            state.active_editor_buffer_id = target_buffer_id;
            state.focus = PaneId::Editor;
            state.mode = Mode::Normal;
            place_focused_cursor(state, line, col);
        }
    }
    outcome
}

/// Vim's `Ctrl-O` invariant: pressing it at the freshest end of the ring
/// records the *live* cursor first, so a follow-up `Ctrl-I` can return.
/// Skipped when the live cursor is already the most recent entry (avoids
/// flooding the ring on a tight back/forward sequence).
fn anchor_current_at_freshest_end(state: &mut AppState, current: JumpEntry) {
    if state.jumplist.cursor() != state.jumplist.len() {
        return;
    }
    if state.jumplist.last() == Some(current) {
        return;
    }
    state.jumplist.push(current);
    // The push left the nav cursor pointing past the just-anchored entry;
    // walk it back one slot so the real step_back below returns the
    // entry *before* the anchor instead of the anchor itself (which is
    // where we already are).
    let _ = state.jumplist.jump_back();
}

/// Walk in `dir` until we hit `Empty` or land on an entry whose
/// `buffer_id` is still resolvable in `state.buffers`. Stale entries
/// (target buffer was closed; the slot was replaced) are silently
/// skipped — without this, a closed-then-reopened workspace would
/// strand `Ctrl-O` on phantom positions.
fn walk_until_live(
    state: &mut AppState,
    dir: JumpDirection,
    focused_buffer_id: usize,
) -> JumpStepOutcome {
    loop {
        let raw = match dir {
            JumpDirection::Back => state.jumplist.jump_back(),
            JumpDirection::Forward => state.jumplist.jump_forward(),
        };
        let outcome = classify_step(raw, focused_buffer_id);
        match outcome {
            JumpStepOutcome::Empty => return outcome,
            JumpStepOutcome::InBuffer { .. } => return outcome,
            JumpStepOutcome::CrossBuffer {
                target_buffer_id, ..
            } => {
                if is_live_editor_buffer(state, target_buffer_id) {
                    return outcome;
                }
                // Stale entry: drop it on the floor and continue the walk.
                // (The ring leaves the entry in place — we just move past
                // it. A future visit will skip it again, which is fine on
                // a 100-entry cap.)
            }
        }
    }
}

fn is_live_editor_buffer(state: &AppState, buffer_id: usize) -> bool {
    buffer_id < state.buffers.len() && buffer_id != state.notes_buffer_id
}

fn place_focused_cursor(state: &mut AppState, line: usize, col: usize) {
    let Some(buf) = state.focused_buffer_mut() else {
        return;
    };
    let total = buf.lines_len();
    if total == 0 {
        return;
    }
    let target_line = line.min(total - 1);
    let visible_chars = buf
        .line_as_string(target_line)
        .chars()
        .take_while(|c| *c != '\n' && *c != '\r')
        .count();
    buf.cursor.line = target_line;
    buf.cursor.col = col.min(visible_chars);
    buf.ensure_visible();
}

fn empty_ring_message(dir: JumpDirection) -> &'static str {
    match dir {
        JumpDirection::Back => "No older jump position",
        JumpDirection::Forward => "No newer jump position",
    }
}

fn classify_step(entry: Option<JumpEntry>, focused_buffer_id: usize) -> JumpStepOutcome {
    let Some(entry) = entry else {
        return JumpStepOutcome::Empty;
    };
    if entry.buffer_id == focused_buffer_id {
        JumpStepOutcome::InBuffer {
            line: entry.line,
            col: entry.col,
        }
    } else {
        JumpStepOutcome::CrossBuffer {
            target_buffer_id: entry.buffer_id,
            line: entry.line,
            col: entry.col,
        }
    }
}
