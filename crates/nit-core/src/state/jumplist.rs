//! State-layer jumplist helpers.
//!
//! The vim-style jumplist *data structure* lives next to the buffer module
//! at `buffer::jumplist` because the ring entries are buffer-local (line,
//! col, buffer_id). This module wraps that ring with the state-layer
//! concerns that only `AppState` can answer:
//!
//! * Which `buffer_id` is the focused editor *right now*?
//! * Are we about to jump cross-buffer (which `AppState` would have to
//!   resolve via `find_editor_buffer_by_path` once the multipane editor
//!   surfaces land)?
//! * Should we restore the live cursor as a forward stash (so `Ctrl-O`
//!   followed by `Ctrl-I` round-trips even when the most-recent push hasn't
//!   been seen yet)?
//!
//! Keeping the data structure in `buffer::jumplist` (single-buffer concern)
//! and the policy here (cross-buffer concern) prevents the buffer module
//! from pulling in `AppState`, which would create a circular module
//! dependency.

use crate::buffer::{JumpEntry, JumpList};
use crate::cursor::Cursor;

/// Outcome of a `Ctrl-O` / `Ctrl-I` step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JumpStepOutcome {
    /// The jumplist returned an entry inside the focused buffer; the
    /// caller should move the cursor to `(line, col)`.
    InBuffer { line: usize, col: usize },
    /// The jumplist returned an entry pointing at a different buffer
    /// than the focused one. Until cross-buffer switching lands, the
    /// caller surfaces this in the status bar and leaves the cursor
    /// alone.
    CrossBuffer {
        target_buffer_id: usize,
        line: usize,
        col: usize,
    },
    /// The ring is empty in the requested direction.
    Empty,
}

/// Push `(buffer_id, cursor)` onto `list`. Mirrors `JumpList::push` for
/// callers that only have a `Cursor` handy.
pub fn push_cursor(list: &mut JumpList, buffer_id: usize, cursor: Cursor) {
    list.push(JumpEntry::new(buffer_id, cursor.line, cursor.col));
}

/// Walk one step back through the ring. Returns `JumpStepOutcome` so the
/// caller can branch on cross-buffer entries without unwrapping the raw
/// `JumpEntry`.
pub fn step_back(list: &mut JumpList, focused_buffer_id: usize) -> JumpStepOutcome {
    classify_step(list.jump_back(), focused_buffer_id)
}

/// Walk one step forward through the ring. See [`step_back`].
pub fn step_forward(list: &mut JumpList, focused_buffer_id: usize) -> JumpStepOutcome {
    classify_step(list.jump_forward(), focused_buffer_id)
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
