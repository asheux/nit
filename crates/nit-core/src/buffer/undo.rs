use super::undo_log::{EditDelta, GroupHint};
use super::Buffer;

impl Buffer {
    pub fn undo(&mut self) -> bool {
        self.undo_log.seal();
        let Some(step) = self.undo_log.pop_undo() else {
            return false;
        };
        let (cursor, dirty_before) = step.apply_undo(&mut self.rope);
        self.cursor = cursor;
        self.dirty = dirty_before;
        self.clear_selection();
        self.record_full_reparse();
        self.clamp_col();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.undo_log.seal();
        let Some(step) = self.undo_log.pop_redo() else {
            return false;
        };
        let (cursor, dirty_after) = step.apply_redo(&mut self.rope);
        self.cursor = cursor;
        self.dirty = dirty_after;
        self.clear_selection();
        self.record_full_reparse();
        self.clamp_col();
        true
    }

    /// Public transaction handle. Multiple edits made between
    /// [`Buffer::begin_undo_group`] and [`Buffer::end_undo_group`] collapse
    /// to a single undo step. Nesting is reference-counted so a helper that
    /// already opens a transaction won't fragment an outer one.
    pub fn begin_undo_group(&mut self) {
        self.undo_log.open_group();
    }

    pub fn end_undo_group(&mut self) {
        self.undo_log.close_group();
    }

    /// Force the in-progress edit group closed. Cursor motions, mode
    /// switches, paste, and any standalone op call this so the next char of
    /// typing starts a fresh word run instead of coalescing.
    pub fn break_undo_group(&mut self) {
        self.end_edit_group();
    }

    /// Tell the history log that the current head reflects the on-disk state
    /// so [`Buffer::is_dirty_relative_to_saved`] can recognise an
    /// undo-to-saved-state as clean. Called by the save path.
    pub fn mark_saved(&mut self) {
        self.undo_log.mark_saved();
    }

    pub fn is_dirty_relative_to_saved(&self) -> bool {
        self.undo_log.dirty_relative_to_save()
    }

    pub(super) fn end_edit_group(&mut self) {
        self.undo_log.seal();
    }

    pub(super) fn record_insert_delta(
        &mut self,
        char_idx: usize,
        text: &str,
        cursor_before: crate::cursor::Cursor,
        hint: GroupHint,
    ) {
        if text.is_empty() {
            return;
        }
        let dirty_before = self.dirty;
        let cursor_after = self.cursor;
        self.undo_log.record(
            EditDelta::Insert {
                char_idx,
                text: text.to_string(),
                cursor_before,
                cursor_after,
            },
            hint,
            dirty_before,
            true,
        );
    }

    pub(super) fn record_delete_delta(
        &mut self,
        char_idx: usize,
        text: &str,
        cursor_before: crate::cursor::Cursor,
        hint: GroupHint,
    ) {
        if text.is_empty() {
            return;
        }
        let dirty_before = self.dirty;
        let cursor_after = self.cursor;
        self.undo_log.record(
            EditDelta::Delete {
                char_idx,
                text: text.to_string(),
                cursor_before,
                cursor_after,
            },
            hint,
            dirty_before,
            true,
        );
    }

    pub(super) fn classify_insert_hint(text: &str) -> GroupHint {
        if text.is_empty() {
            return GroupHint::Atomic;
        }
        let mut chars = text.chars();
        let first = chars.next().expect("non-empty");
        if chars.next().is_some() {
            // Multi-char inserts (paste, indent, smart-newline) are always
            // their own transaction — never coalesce with a word run.
            return GroupHint::Atomic;
        }
        if is_word_continuation(first) {
            GroupHint::InsertWordChar
        } else {
            GroupHint::Atomic
        }
    }
}

/// Word-run membership for insert-grouping. Anything alphanumeric or `_` is
/// part of a word; whitespace, punctuation, and brackets force a new group.
/// This is intentionally narrower than vim's `iskeyword` because we want
/// `foo.bar` to split into two undo steps even though `.` is technically
/// identifier-adjacent in some languages.
fn is_word_continuation(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}
