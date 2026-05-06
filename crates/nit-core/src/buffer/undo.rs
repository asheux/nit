use super::types::Snapshot;
use super::Buffer;

pub(super) const UNDO_LIMIT: usize = 256;

impl Buffer {
    pub fn undo(&mut self) -> bool {
        if let Some(snapshot) = self.undo.pop() {
            self.end_edit_group();
            self.push_redo();
            self.rope = snapshot.rope;
            self.cursor = snapshot.cursor;
            self.dirty = snapshot.dirty;
            self.clear_selection();
            self.record_full_reparse();
            return true;
        }
        false
    }

    pub fn redo(&mut self) -> bool {
        if let Some(snapshot) = self.redo.pop() {
            self.end_edit_group();
            self.push_undo_without_clearing_redo();
            self.rope = snapshot.rope;
            self.cursor = snapshot.cursor;
            self.dirty = snapshot.dirty;
            self.clear_selection();
            self.record_full_reparse();
            return true;
        }
        false
    }

    pub(super) fn push_undo(&mut self) {
        if self.undo.len() >= UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
        });
        self.redo.clear();
    }

    fn push_redo(&mut self) {
        if self.redo.len() >= UNDO_LIMIT {
            self.redo.remove(0);
        }
        self.redo.push(Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
        });
    }

    fn push_undo_without_clearing_redo(&mut self) {
        if self.undo.len() >= UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
        });
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
