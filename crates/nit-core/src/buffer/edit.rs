use super::indent::{is_indent_opener, matching_closer};
use super::Buffer;

impl Buffer {
    pub(super) fn replace_selection_with_str(&mut self, s: &str) {
        let (start, end) = match self.selection_range() {
            Some(range) => range,
            None => return,
        };
        if start >= end {
            self.clear_selection();
            return;
        }

        self.end_edit_group();
        self.push_undo();

        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clear_selection();

        if s.is_empty() {
            self.dirty = true;
            return;
        }

        let idx = self.char_index();
        self.record_insert(idx, s);
        self.rope.insert(idx, s);
        let mut line = self.cursor.line;
        let mut col = self.cursor.col;
        for ch in s.chars() {
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        self.cursor.line = line;
        self.cursor.col = col;
        self.dirty = true;
        self.finish_insert_group();
    }

    pub(super) fn replace_selection_with_newline_preserve_indent(&mut self) {
        let (start, end) = match self.selection_range() {
            Some(range) => range,
            None => return,
        };
        if start >= end {
            self.clear_selection();
            return;
        }

        self.end_edit_group();
        self.push_undo();

        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clear_selection();

        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);
        let idx = self.char_index();

        let char_before = self.last_non_ws_before_cursor();
        let should_increase = char_before.is_some_and(is_indent_opener);
        let char_after = self.first_non_ws_after_cursor();
        let bracket_pair = should_increase
            && char_before
                .and_then(matching_closer)
                .zip(char_after)
                .is_some_and(|(expected, actual)| expected == actual);

        let extra_indent = if should_increase {
            self.indent_unit()
        } else {
            String::new()
        };

        let mut text = String::from("\n");
        text.push_str(&indent);
        text.push_str(&extra_indent);
        if bracket_pair {
            text.push('\n');
            text.push_str(&indent);
        }

        self.record_insert(idx, &text);
        self.rope.insert(idx, &text);
        self.cursor.line += 1;
        self.cursor.col = indent.chars().count() + extra_indent.chars().count();
        self.dirty = true;
        self.finish_insert_group();
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.selection_range().is_some() {
            self.replace_selection_with_str(s);
            return;
        }
        let idx = self.char_index();
        self.record_insert(idx, s);
        self.begin_insert_group(idx);
        self.rope.insert(idx, s);
        let mut line = self.cursor.line;
        let mut col = self.cursor.col;
        for ch in s.chars() {
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        self.cursor.line = line;
        self.cursor.col = col;
        self.dirty = true;
        self.finish_insert_group();
    }

    pub fn open_line_below(&mut self) {
        self.end_edit_group();
        self.push_undo();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);

        let last_char = self.last_non_ws_char_on_line(line);
        let extra_indent = if last_char.is_some_and(|c| is_indent_opener(c) || c == ':') {
            self.indent_unit()
        } else {
            String::new()
        };

        let insert_at = self.rope.line_to_char(line) + self.line_char_len(line);
        let mut text = String::from("\n");
        text.push_str(&indent);
        text.push_str(&extra_indent);
        self.record_insert(insert_at, &text);
        self.rope.insert(insert_at, &text);
        self.cursor.line = line + 1;
        self.cursor.col = indent.chars().count() + extra_indent.chars().count();
        self.dirty = true;
    }

    pub fn open_line_above(&mut self) {
        self.end_edit_group();
        self.push_undo();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = if line > 0 {
            self.line_indent(line.saturating_sub(1))
        } else {
            String::new()
        };
        let idx = self.rope.line_to_char(line);
        let mut text = indent.clone();
        text.push('\n');
        self.record_insert(idx, &text);
        self.rope.insert(idx, &text);
        self.cursor.line = line;
        self.cursor.col = indent.chars().count();
        self.dirty = true;
    }

    pub fn paste_line_above(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.end_edit_group();
        self.push_undo();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let idx = self.rope.line_to_char(line);
        self.record_insert(idx, text);
        self.rope.insert(idx, text);
        self.cursor.line = line;
        self.cursor.col = 0;
        self.dirty = true;
        self.clamp_col();
    }

    pub fn paste_line_below(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.end_edit_group();
        self.push_undo();
        let total = self.rope.len_lines();
        let line = self.cursor.line.min(total.saturating_sub(1));
        let idx = if line + 1 < total {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        };
        let mut insert_text = String::new();
        if idx > 0 && self.rope.char(idx.saturating_sub(1)) != '\n' {
            insert_text.push('\n');
        }
        insert_text.push_str(text);
        self.record_insert(idx, &insert_text);
        self.rope.insert(idx, &insert_text);
        self.cursor.line = (line + 1).min(self.rope.len_lines().saturating_sub(1));
        self.cursor.col = 0;
        self.dirty = true;
        self.clamp_col();
    }

    pub fn insert_char(&mut self, c: char) {
        if self.selection_range().is_some() {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            self.replace_selection_with_str(s);
            return;
        }
        let idx = self.char_index();
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        self.record_insert(idx, s);
        self.begin_insert_group(idx);
        self.rope.insert_char(idx, c);
        self.cursor.col += 1;
        self.dirty = true;
        self.finish_insert_group();
    }

    pub fn insert_tab(&mut self) {
        self.insert_char('\t');
    }

    pub fn insert_newline(&mut self) {
        if self.selection_range().is_some() {
            self.replace_selection_with_newline_preserve_indent();
            return;
        }
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);
        let idx = self.char_index();

        let char_before = self.last_non_ws_before_cursor();
        let should_increase = char_before.is_some_and(is_indent_opener);

        // Bracket pair expansion: cursor between matching brackets like {|}
        let char_after = self.first_non_ws_after_cursor();
        let bracket_pair = should_increase
            && char_before
                .and_then(matching_closer)
                .zip(char_after)
                .is_some_and(|(expected, actual)| expected == actual);

        let extra_indent = if should_increase {
            self.indent_unit()
        } else {
            String::new()
        };

        let mut text = String::from("\n");
        text.push_str(&indent);
        text.push_str(&extra_indent);
        if bracket_pair {
            text.push('\n');
            text.push_str(&indent);
        }

        self.record_insert(idx, &text);
        self.begin_insert_group(idx);
        self.rope.insert(idx, &text);
        self.cursor.line += 1;
        self.cursor.col = indent.chars().count() + extra_indent.chars().count();
        self.dirty = true;
        self.finish_insert_group();
    }

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

        let is_word = super::cursor_motion::is_word_char;
        let mut idx = end;
        while idx > 0 && self.rope.char(idx - 1).is_whitespace() {
            idx = idx.saturating_sub(1);
        }
        if idx == 0 {
            return;
        }
        if is_word(self.rope.char(idx - 1)) {
            while idx > 0 && is_word(self.rope.char(idx - 1)) {
                idx = idx.saturating_sub(1);
            }
        } else {
            while idx > 0 {
                let ch = self.rope.char(idx - 1);
                if ch.is_whitespace() || is_word(ch) {
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

        let is_word = super::cursor_motion::is_word_char;
        let mut idx = start;
        if self.rope.char(idx).is_whitespace() {
            while idx < len && self.rope.char(idx).is_whitespace() {
                idx += 1;
            }
        } else if is_word(self.rope.char(idx)) {
            while idx < len && is_word(self.rope.char(idx)) {
                idx += 1;
            }
        } else {
            while idx < len {
                let ch = self.rope.char(idx);
                if ch.is_whitespace() || is_word(ch) {
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

    /// vim `J`: join the next line onto the current line with a single space.
    pub fn join_lines(&mut self) {
        self.end_edit_group();
        let total = self.rope.len_lines();
        let line = self.cursor.line;
        if line + 1 >= total {
            return;
        }
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let newline_idx = line_start + line_len;
        let next_start = self.rope.line_to_char(line + 1);
        let next_len = self.line_char_len(line + 1);
        let mut ws_end = 0;
        while ws_end < next_len {
            let c = self.rope.char(next_start + ws_end);
            if c != ' ' && c != '\t' {
                break;
            }
            ws_end += 1;
        }
        let end_idx = next_start + ws_end;
        self.push_undo();
        if end_idx > newline_idx {
            self.record_delete(newline_idx, end_idx);
            self.rope.remove(newline_idx..end_idx);
        }
        let has_next_content = ws_end < next_len;
        let our_line_has_content = line_len > 0;
        let mut final_col = line_len;
        if has_next_content && our_line_has_content {
            let insert_idx = line_start + line_len;
            self.record_insert(insert_idx, " ");
            self.rope.insert_char(insert_idx, ' ');
            final_col = line_len;
        } else if !our_line_has_content {
            final_col = 0;
        }
        self.cursor.line = line;
        self.cursor.col = final_col;
        self.dirty = true;
        self.clamp_col();
    }

    /// vim `~`: toggle case of the character under the cursor and advance.
    pub fn toggle_case_char(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let idx = self.char_index();
        if idx >= len {
            return;
        }
        let c = self.rope.char(idx);
        if c == '\n' {
            return;
        }
        let new_c = if c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else if c.is_ascii_lowercase() {
            c.to_ascii_uppercase()
        } else {
            c
        };
        if new_c != c {
            self.push_undo();
            self.record_delete(idx, idx + 1);
            self.rope.remove(idx..idx + 1);
            let mut buf = [0u8; 4];
            let s = new_c.encode_utf8(&mut buf);
            self.record_insert(idx, s);
            self.rope.insert(idx, s);
            self.dirty = true;
        }
        let line_len = self.line_char_len(self.cursor.line);
        if self.cursor.col < line_len {
            self.cursor.col += 1;
        }
    }

    /// vim `r<c>`: replace the character under the cursor with `new_c`.
    /// Does nothing if the cursor is on a newline or past end of buffer.
    pub fn replace_char(&mut self, new_c: char) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let idx = self.char_index();
        if idx >= len {
            return;
        }
        let c = self.rope.char(idx);
        if c == '\n' {
            return;
        }
        self.push_undo();
        self.record_delete(idx, idx + 1);
        self.rope.remove(idx..idx + 1);
        let mut buf = [0u8; 4];
        let s = new_c.encode_utf8(&mut buf);
        self.record_insert(idx, s);
        self.rope.insert(idx, s);
        self.dirty = true;
    }
}
