use super::types::Snapshot;
use super::Buffer;

pub(super) const UNDO_LIMIT: usize = 256;

impl Buffer {
    pub fn undo(&mut self) -> bool {
        self.pop_and_swap(true)
    }

    pub fn redo(&mut self) -> bool {
        self.pop_and_swap(false)
    }

    pub(super) fn push_undo(&mut self) {
        let snap = self.snapshot();
        push_snapshot(&mut self.undo, snap);
        self.redo.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
        }
    }

    fn pop_and_swap(&mut self, pop_undo: bool) -> bool {
        let popped = if pop_undo {
            self.undo.pop()
        } else {
            self.redo.pop()
        };
        let Some(snapshot) = popped else {
            return false;
        };
        self.end_edit_group();
        // Push current state to the *opposite* stack so the swap is reversible.
        let mirror = self.snapshot();
        if pop_undo {
            push_snapshot(&mut self.redo, mirror);
        } else {
            push_snapshot(&mut self.undo, mirror);
        }
        self.rope = snapshot.rope;
        self.cursor = snapshot.cursor;
        self.dirty = snapshot.dirty;
        self.clear_selection();
        self.record_full_reparse();
        true
    }

    pub(super) fn begin_insert_group(&mut self, idx: usize) {
        let start_new = match self.last_edit {
            Some(meta) => meta.kind != super::types::EditKind::Insert || meta.cursor_index != idx,
            None => true,
        };
        if start_new {
            self.push_undo();
        }
    }

    pub(super) fn finish_insert_group(&mut self) {
        let cursor_index = self.char_index();
        self.last_edit = Some(super::types::EditMeta {
            kind: super::types::EditKind::Insert,
            cursor_index,
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
