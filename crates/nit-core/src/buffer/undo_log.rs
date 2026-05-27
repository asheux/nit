//! Delta-based per-buffer edit log.
//!
//! Each atomic mutation (one inserted run, one deleted span) is recorded as
//! one [`EditDelta`]. Deltas share a group id when they belong to the same
//! transaction — typing within a word, a multi-line block indent, an
//! auto-paired bracket. Undo replays the deltas of the most recent group in
//! reverse; redo re-applies them forward.
//!
//! The log replaces the prior full-rope snapshot stack. Memory cost scales
//! with the bytes actually edited rather than the size of the file.
//!
//! Group boundaries are sealed explicitly by callers via [`UndoLog::seal`]
//! (cursor motion, mode switch, save, large paste) and implicitly by the
//! word-boundary heuristic in [`UndoLog::push_insert_char`] (whitespace or
//! a position discontinuity opens a new group).

use ropey::Rope;

use crate::cursor::Cursor;

/// Cap on retained transaction groups per stack. The oldest is dropped on
/// overflow so a long editing session keeps the redo path live without
/// unbounded memory growth.
pub(super) const UNDO_GROUP_LIMIT: usize = 1024;

/// One atomic delta. Both variants carry the post-edit cursor so undo can
/// restore the caret to exactly where the user expects after stepping back
/// through a transaction's deltas.
#[derive(Clone, Debug)]
pub(super) enum EditDelta {
    Insert {
        char_idx: usize,
        text: String,
        cursor_before: Cursor,
        cursor_after: Cursor,
    },
    Delete {
        char_idx: usize,
        text: String,
        cursor_before: Cursor,
        cursor_after: Cursor,
    },
}

impl EditDelta {
    fn apply_undo(&self, rope: &mut Rope) -> Cursor {
        match self {
            EditDelta::Insert {
                char_idx,
                text,
                cursor_before,
                ..
            } => {
                let end = char_idx + text.chars().count();
                rope.remove(*char_idx..end);
                *cursor_before
            }
            EditDelta::Delete {
                char_idx,
                text,
                cursor_before,
                ..
            } => {
                rope.insert(*char_idx, text);
                *cursor_before
            }
        }
    }

    fn apply_redo(&self, rope: &mut Rope) -> Cursor {
        match self {
            EditDelta::Insert {
                char_idx,
                text,
                cursor_after,
                ..
            } => {
                rope.insert(*char_idx, text);
                *cursor_after
            }
            EditDelta::Delete {
                char_idx,
                text,
                cursor_after,
                ..
            } => {
                let end = char_idx + text.chars().count();
                rope.remove(*char_idx..end);
                *cursor_after
            }
        }
    }
}

/// Grouping hint paired with each delta so the log knows whether to coalesce
/// with the prior delta in the open group or start a fresh one.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum GroupHint {
    /// A continuation of the user's current "word run" — typing a non-word
    /// character (whitespace, punct) breaks the run.
    InsertWordChar,
    /// Continuation of a backspace run (cursor walking back through chars).
    DeleteBack,
    /// Continuation of a forward-delete run (cursor stays put as chars to
    /// the right disappear).
    DeleteForward,
    /// Caller has opened an explicit transaction via [`UndoLog::open_group`].
    /// All deltas pushed until the matching `close_group` share one id.
    Explicit,
    /// A standalone edit that must not coalesce with anything (paste, line
    /// op, replace, auto-pair, smart-newline).
    Atomic,
}

/// Per-buffer undo / redo state. Holds two stacks of transaction groups; each
/// group is a `Vec<EditDelta>` applied as one undo step. The `open_group_id`
/// is `Some(id)` while a transaction is in progress and `None` once sealed.
#[derive(Clone, Debug, Default)]
pub(super) struct UndoLog {
    undo: Vec<Group>,
    redo: Vec<Group>,
    open_group_id: Option<u64>,
    next_group_id: u64,
    last_hint: Option<GroupHint>,
    last_char_idx: Option<usize>,
    explicit_depth: u32,
    saved_group_id: Option<u64>,
}

#[derive(Clone, Debug)]
struct Group {
    id: u64,
    deltas: Vec<EditDelta>,
    dirty_before: bool,
    dirty_after: bool,
}

impl UndoLog {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn mark_saved(&mut self) {
        self.saved_group_id = self.undo.last().map(|g| g.id);
    }

    pub(super) fn dirty_relative_to_save(&self) -> bool {
        let head = self.undo.last().map(|g| g.id);
        head != self.saved_group_id
    }

    /// Open an explicit transaction. Nested opens are reference-counted so a
    /// caller can wrap a sub-helper that itself opens a group without
    /// fragmenting the outer transaction.
    pub(super) fn open_group(&mut self) {
        self.explicit_depth = self.explicit_depth.saturating_add(1);
        if self.explicit_depth == 1 {
            self.seal_open_group();
            self.start_new_group(true);
            self.last_hint = Some(GroupHint::Explicit);
        }
    }

    pub(super) fn close_group(&mut self) {
        if self.explicit_depth == 0 {
            return;
        }
        self.explicit_depth -= 1;
        if self.explicit_depth == 0 {
            self.seal_open_group();
            self.last_hint = None;
            self.last_char_idx = None;
        }
    }

    /// Seal the currently-open transaction (if any) without affecting depth
    /// counters. Idempotent. Called on cursor motion, mode switch, paste, etc.
    pub(super) fn seal(&mut self) {
        if self.explicit_depth > 0 {
            // Inside an explicit group — don't seal until the matching close.
            return;
        }
        self.seal_open_group();
        self.last_hint = None;
        self.last_char_idx = None;
    }

    pub(super) fn record(
        &mut self,
        delta: EditDelta,
        hint: GroupHint,
        dirty_before: bool,
        dirty_after: bool,
    ) {
        // Any fresh edit invalidates the redo path — vim's branching rule.
        self.redo.clear();

        let should_open_new = self.should_open_new_group(hint, &delta);
        if should_open_new {
            self.seal_open_group();
            self.start_new_group(dirty_before);
        }
        let group = self
            .undo
            .last_mut()
            .expect("start_new_group keeps stack non-empty");
        group.deltas.push(delta.clone());
        group.dirty_after = dirty_after;

        self.last_hint = Some(hint);
        self.last_char_idx = Some(post_delta_anchor(&delta));
        if matches!(hint, GroupHint::Atomic) {
            // Atomic ops are inherently a single-delta group — close it
            // immediately so the next edit opens its own group.
            self.seal_open_group();
        }
    }

    pub(super) fn pop_undo(&mut self) -> Option<UndoStep> {
        self.seal_open_group();
        let group = self.undo.pop()?;
        let step = UndoStep {
            deltas: group.deltas.clone(),
            dirty_before: group.dirty_before,
        };
        self.redo.push(group);
        bound_stack(&mut self.redo);
        self.last_hint = None;
        self.last_char_idx = None;
        Some(step)
    }

    pub(super) fn pop_redo(&mut self) -> Option<UndoStep> {
        self.seal_open_group();
        let group = self.redo.pop()?;
        let step = UndoStep {
            deltas: group.deltas.clone(),
            dirty_before: group.dirty_after,
        };
        self.undo.push(group);
        bound_stack(&mut self.undo);
        self.last_hint = None;
        self.last_char_idx = None;
        Some(step)
    }

    fn seal_open_group(&mut self) {
        if let Some(open_id) = self.open_group_id.take() {
            if let Some(group) = self.undo.last() {
                if group.id == open_id && group.deltas.is_empty() {
                    // An empty group means the caller opened then closed
                    // without producing any delta — drop it.
                    self.undo.pop();
                }
            }
        }
    }

    fn start_new_group(&mut self, dirty_before: bool) {
        let id = self.next_group_id;
        self.next_group_id = self.next_group_id.wrapping_add(1);
        self.undo.push(Group {
            id,
            deltas: Vec::new(),
            dirty_before,
            dirty_after: dirty_before,
        });
        self.open_group_id = Some(id);
        bound_stack(&mut self.undo);
    }

    fn should_open_new_group(&self, hint: GroupHint, delta: &EditDelta) -> bool {
        if self.open_group_id.is_none() {
            return true;
        }
        match hint {
            GroupHint::Atomic => true,
            GroupHint::Explicit => {
                // While an explicit transaction is open, every delta joins it.
                self.explicit_depth == 0
            }
            GroupHint::InsertWordChar => {
                if !matches!(self.last_hint, Some(GroupHint::InsertWordChar)) {
                    return true;
                }
                position_breaks_insert_run(self.last_char_idx, delta)
            }
            GroupHint::DeleteBack => {
                if !matches!(self.last_hint, Some(GroupHint::DeleteBack)) {
                    return true;
                }
                position_breaks_delete_back(self.last_char_idx, delta)
            }
            GroupHint::DeleteForward => {
                if !matches!(self.last_hint, Some(GroupHint::DeleteForward)) {
                    return true;
                }
                position_breaks_delete_forward(self.last_char_idx, delta)
            }
        }
    }
}

/// Replay payload for [`super::Buffer::apply_undo_step`]. Cloning the inner
/// deltas lets the buffer mutate the rope through them while the group stays
/// intact on the redo stack.
pub(super) struct UndoStep {
    pub(super) deltas: Vec<EditDelta>,
    pub(super) dirty_before: bool,
}

impl UndoStep {
    pub(super) fn apply_undo(self, rope: &mut Rope) -> (Cursor, bool) {
        let mut final_cursor = Cursor::default();
        for delta in self.deltas.iter().rev() {
            final_cursor = delta.apply_undo(rope);
        }
        (final_cursor, self.dirty_before)
    }

    pub(super) fn apply_redo(self, rope: &mut Rope) -> (Cursor, bool) {
        let mut final_cursor = Cursor::default();
        for delta in self.deltas.iter() {
            final_cursor = delta.apply_redo(rope);
        }
        (final_cursor, self.dirty_before)
    }
}

/// Insert runs continue while each new character lands one past the previous
/// insertion. A cursor jump (e.g. `move_word_forward` placed the caret
/// somewhere else, then the next char arrived) breaks the run.
fn position_breaks_insert_run(prev_anchor: Option<usize>, delta: &EditDelta) -> bool {
    let Some(prev) = prev_anchor else {
        return true;
    };
    match delta {
        EditDelta::Insert { char_idx, .. } => *char_idx != prev,
        _ => true,
    }
}

/// Backspace runs continue while each new delete starts one char to the left
/// of the previous one (cursor walked back as content vanished).
fn position_breaks_delete_back(prev_anchor: Option<usize>, delta: &EditDelta) -> bool {
    let Some(prev) = prev_anchor else {
        return true;
    };
    match delta {
        EditDelta::Delete { char_idx, text, .. } => {
            let removed = text.chars().count();
            char_idx + removed != prev
        }
        _ => true,
    }
}

/// Forward-delete runs continue while each new delete starts at the same
/// char index as before (the right-hand side shifts left under a stationary
/// cursor).
fn position_breaks_delete_forward(prev_anchor: Option<usize>, delta: &EditDelta) -> bool {
    let Some(prev) = prev_anchor else {
        return true;
    };
    match delta {
        EditDelta::Delete { char_idx, .. } => *char_idx != prev,
        _ => true,
    }
}

/// Anchor recorded per delta so the next delta's hint can compare against it.
/// For inserts: the index immediately after the inserted text — the next char
/// the user types should land there to extend the run. For deletes: the index
/// remaining in the rope where the next delete should continue from.
fn post_delta_anchor(delta: &EditDelta) -> usize {
    match delta {
        EditDelta::Insert { char_idx, text, .. } => char_idx + text.chars().count(),
        EditDelta::Delete { char_idx, .. } => *char_idx,
    }
}

fn bound_stack(stack: &mut Vec<Group>) {
    while stack.len() > UNDO_GROUP_LIMIT {
        stack.remove(0);
    }
}
