use super::types::{BufferEdit, BufferPoint};
use super::Buffer;

impl Buffer {
    pub fn take_pending_edits(&mut self) -> Vec<BufferEdit> {
        std::mem::take(&mut self.pending_edits)
    }

    pub fn take_full_reparse(&mut self) -> bool {
        let flag = self.full_reparse;
        self.full_reparse = false;
        flag
    }

    pub(super) fn record_insert(&mut self, start_char: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let start_byte = self.rope.char_to_byte(start_char);
        let start_point = self.point_from_char_index(start_char);
        let (new_end_byte, new_end_point) = advance_point(start_byte, start_point, text);
        self.push_edit(BufferEdit {
            start_byte,
            old_end_byte: start_byte,
            new_end_byte,
            start_point,
            old_end_point: start_point,
            new_end_point,
        });
    }

    pub(super) fn record_delete(&mut self, start_char: usize, end_char: usize) {
        if start_char >= end_char {
            return;
        }
        let start_byte = self.rope.char_to_byte(start_char);
        let old_end_byte = self.rope.char_to_byte(end_char);
        let start_point = self.point_from_char_index(start_char);
        let old_end_point = self.point_from_char_index(end_char);
        self.push_edit(BufferEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte,
            start_point,
            old_end_point,
            new_end_point: start_point,
        });
    }

    fn push_edit(&mut self, edit: BufferEdit) {
        self.pending_edits.push(edit);
        self.version = self.version.wrapping_add(1);
    }

    pub(super) fn record_full_reparse(&mut self) {
        self.pending_edits.clear();
        self.full_reparse = true;
        self.version = self.version.wrapping_add(1);
    }

    fn point_from_char_index(&self, idx: usize) -> BufferPoint {
        let line = self.rope.char_to_line(idx);
        let line_start_char = self.rope.line_to_char(line);
        let line_start_byte = self.rope.char_to_byte(line_start_char);
        let byte = self.rope.char_to_byte(idx);
        BufferPoint {
            row: line,
            column: byte.saturating_sub(line_start_byte),
        }
    }

    pub(super) fn set_cursor_from_char_index(&mut self, idx: usize) {
        let line = self.rope.char_to_line(idx);
        let line_start = self.rope.line_to_char(line);
        self.cursor.line = line;
        self.cursor.col = idx.saturating_sub(line_start);
        self.clamp_col();
    }
}

fn advance_point(start_byte: usize, start_point: BufferPoint, text: &str) -> (usize, BufferPoint) {
    let mut row = start_point.row;
    let mut column = start_point.column;
    let mut byte = start_byte;
    for b in text.bytes() {
        byte += 1;
        if b == b'\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (byte, BufferPoint { row, column })
}
