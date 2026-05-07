use super::super::cursor_motion::is_word_char;
use super::super::Buffer;

impl Buffer {
    pub fn backspace(&mut self) {
        self.end_edit_group();
        if self.cursor.col > 0 {
            let idx = self.char_index();
            if idx > 0 {
                self.record_delete(idx - 1, idx);
                self.push_undo();
                self.rope.remove(idx - 1..idx);
                self.cursor.col -= 1;
                self.dirty = true;
            }
        } else if self.cursor.line > 0 {
            let prev_len = self.line_char_len(self.cursor.line - 1);
            let idx = self.char_index();
            if idx > 0 {
                self.record_delete(idx - 1, idx);
                self.push_undo();
                self.rope.remove(idx - 1..idx);
                self.cursor.line -= 1;
                self.cursor.col = prev_len;
                self.dirty = true;
            }
        }
    }

    pub fn delete_word_back(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let end = self.char_index().min(len);
        if end == 0 || len == 0 {
            return;
        }

        let mut idx = end;
        while idx > 0 && self.rope.char(idx - 1).is_whitespace() {
            idx = idx.saturating_sub(1);
        }
        if idx == 0 {
            return;
        }
        if is_word_char(self.rope.char(idx - 1)) {
            while idx > 0 && is_word_char(self.rope.char(idx - 1)) {
                idx = idx.saturating_sub(1);
            }
        } else {
            while idx > 0 {
                let ch = self.rope.char(idx - 1);
                if ch.is_whitespace() || is_word_char(ch) {
                    break;
                }
                idx = idx.saturating_sub(1);
            }
        }

        let start = idx;
        if start >= end {
            return;
        }
        self.record_delete(start, end);
        self.push_undo();
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.dirty = true;
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
            self.record_delete(idx, idx + 1);
            self.push_undo();
            self.rope.remove(idx..idx + 1);
            self.dirty = true;
        }
    }

    pub fn delete_word_forward(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let start = self.char_index().min(len);
        if start >= len || len == 0 {
            return;
        }

        let mut idx = start;
        if self.rope.char(idx).is_whitespace() {
            while idx < len && self.rope.char(idx).is_whitespace() {
                idx += 1;
            }
        } else if is_word_char(self.rope.char(idx)) {
            while idx < len && is_word_char(self.rope.char(idx)) {
                idx += 1;
            }
        } else {
            while idx < len {
                let ch = self.rope.char(idx);
                if ch.is_whitespace() || is_word_char(ch) {
                    break;
                }
                idx += 1;
            }
        }

        let end = idx;
        if end <= start {
            return;
        }
        self.record_delete(start, end);
        self.push_undo();
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.dirty = true;
        self.clamp_col();
    }

    pub fn delete_line(&mut self) {
        self.end_edit_group();
        let total = self.rope.len_lines();
        if total == 0 {
            return;
        }
        self.push_undo();
        let line = self.cursor.line.min(total.saturating_sub(1));
        let start = self.rope.line_to_char(line);
        let end = if line + 1 < total {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        };
        if end > start {
            self.record_delete(start, end);
            self.rope.remove(start..end);
        }
        let new_total = self.rope.len_lines();
        if new_total == 0 {
            self.cursor.line = 0;
            self.cursor.col = 0;
        } else if self.cursor.line >= new_total {
            self.cursor.line = new_total.saturating_sub(1);
            self.cursor.col = 0;
        } else {
            self.cursor.col = 0;
        }
        self.dirty = true;
        self.clamp_col();
    }

    /// vim `D`: delete from cursor to end of line.
    pub fn delete_to_end(&mut self) {
        self.end_edit_group();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
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
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let indent = self.line_indent(line);
        let indent_chars = indent.chars().count().min(line_len);
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
}
