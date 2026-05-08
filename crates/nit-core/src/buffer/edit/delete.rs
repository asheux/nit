use ropey::Rope;

use super::super::cursor_motion::is_word_char;
use super::super::Buffer;

impl Buffer {
    pub fn backspace(&mut self) {
        self.end_edit_group();
        let idx = self.char_index();
        if idx == 0 {
            return;
        }
        if self.cursor.col > 0 {
            self.delete_range_with_undo(idx - 1, idx);
            self.cursor.col -= 1;
            return;
        }
        if self.cursor.line == 0 {
            return;
        }
        let prev_len = self.line_char_len(self.cursor.line - 1);
        self.delete_range_with_undo(idx - 1, idx);
        self.cursor.line -= 1;
        self.cursor.col = prev_len;
    }

    pub fn delete_word_back(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let end = self.char_index().min(len);
        if end == 0 {
            return;
        }
        let start = scan_word_start_back(&self.rope, end);
        if start >= end {
            return;
        }
        self.delete_range_with_undo(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
    }

    pub fn delete_forward(&mut self) {
        self.end_edit_group();
        let idx = self.char_index();
        let line_len = self.line_char_len(self.cursor.line);
        // At end of line, the next char is the newline joining to the next line.
        let on_char = self.cursor.col < line_len;
        let at_newline = self.cursor.line + 1 < self.rope.len_lines();
        if on_char || at_newline {
            self.delete_range_with_undo(idx, idx + 1);
        }
    }

    pub fn delete_word_forward(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let start = self.char_index().min(len);
        if start >= len {
            return;
        }
        let end = scan_word_end_forward(&self.rope, start, len);
        if end <= start {
            return;
        }
        self.delete_range_with_undo(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
    }

    pub fn delete_line(&mut self) {
        self.end_edit_group();
        let total = self.rope.len_lines();
        if total == 0 {
            return;
        }
        self.push_undo();
        let line = self.cursor.line.min(total - 1);
        let start = self.rope.line_to_char(line);
        let end = self.line_end_char_index(line);
        if end > start {
            self.record_delete(start, end);
            self.rope.remove(start..end);
        }
        let new_total = self.rope.len_lines();
        self.cursor.col = 0;
        if new_total == 0 {
            self.cursor.line = 0;
        } else if self.cursor.line >= new_total {
            self.cursor.line = new_total - 1;
        }
        self.dirty = true;
        self.clamp_col();
    }

    /// vim `D`: delete from cursor to end of line.
    pub fn delete_to_end(&mut self) {
        self.end_edit_group();
        let line = self.clamped_cursor_line();
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let col = self.cursor.col.min(line_len);
        let start = line_start + col;
        let end = line_start + line_len;
        if end <= start {
            return;
        }
        self.push_undo();
        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.dirty = true;
        self.clamp_col();
    }

    /// vim `S` / `cc`: clear the current line, preserving leading indent.
    /// Caller is expected to switch to insert mode afterwards.
    pub fn substitute_line(&mut self) {
        self.end_edit_group();
        let line = self.clamped_cursor_line();
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let indent_chars = self.line_indent(line).chars().count().min(line_len);
        if line_len > indent_chars {
            let start = line_start + indent_chars;
            let end = line_start + line_len;
            self.push_undo();
            self.record_delete(start, end);
            self.rope.remove(start..end);
            self.dirty = true;
        }
        self.cursor.col = indent_chars;
    }

    pub(super) fn delete_range_with_undo(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.record_delete(start, end);
        self.push_undo();
        self.rope.remove(start..end);
        self.dirty = true;
    }

    fn line_end_char_index(&self, line: usize) -> usize {
        if line + 1 < self.rope.len_lines() {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        }
    }
}

fn scan_word_start_back(rope: &Rope, from: usize) -> usize {
    let mut idx = from;
    while idx > 0 && rope.char(idx - 1).is_whitespace() {
        idx -= 1;
    }
    if idx == 0 {
        return idx;
    }
    if is_word_char(rope.char(idx - 1)) {
        while idx > 0 && is_word_char(rope.char(idx - 1)) {
            idx -= 1;
        }
    } else {
        while idx > 0 {
            let ch = rope.char(idx - 1);
            if ch.is_whitespace() || is_word_char(ch) {
                break;
            }
            idx -= 1;
        }
    }
    idx
}

fn scan_word_end_forward(rope: &Rope, start: usize, len: usize) -> usize {
    let mut idx = start;
    let first = rope.char(idx);
    if first.is_whitespace() {
        while idx < len && rope.char(idx).is_whitespace() {
            idx += 1;
        }
    } else if is_word_char(first) {
        while idx < len && is_word_char(rope.char(idx)) {
            idx += 1;
        }
    } else {
        while idx < len {
            let ch = rope.char(idx);
            if ch.is_whitespace() || is_word_char(ch) {
                break;
            }
            idx += 1;
        }
    }
    idx
}
