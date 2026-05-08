use super::types::{EditKind, EditMeta, Snapshot};
use super::Buffer;

/// Maximum snapshots retained per stack. The oldest entry is dropped on
/// overflow so a long editing session keeps the redo path live without
/// unbounded memory growth.
pub(super) const UNDO_LIMIT: usize = 256;

impl Buffer {
    pub fn undo(&mut self) -> bool {
        self.swap_with_history(true)
    }

    pub fn redo(&mut self) -> bool {
        self.swap_with_history(false)
    }

    pub(super) fn push_undo(&mut self) {
        let snap = self.snapshot();
        push_snapshot(&mut self.undo, snap);
        // A fresh edit invalidates the redo stack — the user is now branching
        // away from any previously-undone state.
        self.redo.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
        }
    }

    fn swap_with_history(&mut self, pop_undo: bool) -> bool {
        let popped = if pop_undo {
            self.undo.pop()
        } else {
            self.redo.pop()
        };
        let Some(snapshot) = popped else {
            return false;
        };
        self.end_edit_group();
        // Push current state onto the *opposite* stack so the swap is reversible.
        let mirror = self.snapshot();
        let target = if pop_undo {
            &mut self.redo
        } else {
            &mut self.undo
        };
        push_snapshot(target, mirror);
        self.rope = snapshot.rope;
        self.cursor = snapshot.cursor;
        self.dirty = snapshot.dirty;
        self.clear_selection();
        self.record_full_reparse();
        true
    }

    pub(super) fn begin_insert_group(&mut self, idx: usize) {
        let start_new = match self.last_edit {
            Some(meta) => meta.kind != EditKind::Insert || meta.cursor_index != idx,
            None => true,
        };
        if start_new {
            self.push_undo();
        }
    }

    pub(super) fn finish_insert_group(&mut self) {
        self.last_edit = Some(EditMeta {
            kind: EditKind::Insert,
            cursor_index: self.char_index(),
        });
    }

    pub(super) fn end_edit_group(&mut self) {
        self.last_edit = None;
    }

    pub fn break_undo_group(&mut self) {
        self.end_edit_group();
    }
}

fn push_snapshot(stack: &mut Vec<Snapshot>, snap: Snapshot) {
    if stack.len() >= UNDO_LIMIT {
        stack.remove(0);
    }
    stack.push(snap);
}
