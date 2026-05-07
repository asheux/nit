use super::super::indent::{is_indent_opener, matching_closer};
use super::super::Buffer;

impl Buffer {
    pub(in crate::buffer) fn replace_selection_with_str(&mut self, s: &str) {
        let Some((start, end)) = self.selection_range() else {
            return;
        };
        if start >= end {
            self.clear_selection();
            return;
        }

        self.end_edit_group();
        self.push_undo();
        self.apply_selection_removal(start, end);

        if s.is_empty() {
            self.dirty = true;
            return;
        }

        let idx = self.char_index();
        self.record_insert(idx, s);
        self.rope.insert(idx, s);
        self.advance_cursor_through(s);
        self.dirty = true;
        self.finish_insert_group();
    }

    pub(in crate::buffer) fn replace_selection_with_newline_preserve_indent(&mut self) {
        let Some((start, end)) = self.selection_range() else {
            return;
        };
        if start >= end {
            self.clear_selection();
            return;
        }

        self.end_edit_group();
        self.push_undo();
        self.apply_selection_removal(start, end);

        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);
        let idx = self.char_index();
        let (text, cursor_col) = self.build_indented_newline(&indent);

        self.record_insert(idx, &text);
        self.rope.insert(idx, &text);
        self.cursor.line += 1;
        self.cursor.col = cursor_col;
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
        self.advance_cursor_through(s);
        self.dirty = true;
        self.finish_insert_group();
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
        let (text, cursor_col) = self.build_indented_newline(&indent);

        self.record_insert(idx, &text);
        self.begin_insert_group(idx);
        self.rope.insert(idx, &text);
        self.cursor.line += 1;
        self.cursor.col = cursor_col;
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

    fn advance_cursor_through(&mut self, s: &str) {
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
    }

    /// Compute the post-newline insertion text and resulting cursor column for
    /// an indent-aware newline at the current cursor position.
    /// Returns `(text_to_insert, cursor_col_after)`.
    fn build_indented_newline(&self, indent: &str) -> (String, usize) {
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
        text.push_str(indent);
        text.push_str(&extra_indent);
        if bracket_pair {
            text.push('\n');
            text.push_str(indent);
        }
        let cursor_col = indent.chars().count() + extra_indent.chars().count();
        (text, cursor_col)
    }
}
