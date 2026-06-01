use super::super::Buffer;

impl Buffer {
    /// vim `J`: join the next line onto the current line, replacing the
    /// joining newline + leading whitespace with a single space when both
    /// lines have non-whitespace content. Recorded as one transaction so a
    /// single undo restores both lines.
    pub fn join_lines(&mut self) {
        self.end_edit_group();
        let line = self.cursor.line;
        if line + 1 >= self.rope.len_lines() {
            return;
        }
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let join_at = line_start + line_len;
        let next_start = self.rope.line_to_char(line + 1);
        let next_len = self.line_char_len(line + 1);
        let leading_ws = count_leading_blanks(&self.rope, next_start, next_len);
        let strip_end = next_start + leading_ws;

        self.begin_undo_group();
        let before = self.cursor;
        if strip_end > join_at {
            let removed = self.rope.slice(join_at..strip_end).to_string();
            self.record_delete(join_at, strip_end);
            self.rope.remove(join_at..strip_end);
            self.record_delete_delta(
                join_at,
                &removed,
                before,
                super::super::undo_log::GroupHint::Explicit,
            );
        }
        let next_has_content = leading_ws < next_len;
        let line_has_content = line_len > 0;
        if next_has_content && line_has_content {
            self.record_insert(join_at, " ");
            self.rope.insert_char(join_at, ' ');
            self.record_insert_delta(
                join_at,
                " ",
                before,
                super::super::undo_log::GroupHint::Explicit,
            );
        }
        self.cursor.line = line;
        self.cursor.col = if line_has_content { line_len } else { 0 };
        self.dirty = true;
        self.clamp_col();
        self.end_undo_group();
    }

    /// vim `~`: toggle case of the character under the cursor and advance.
    pub fn toggle_case_char(&mut self) {
        if self.transform_char_at_cursor(toggle_ascii_case) {
            let line_len = self.line_char_len(self.cursor.line);
            if self.cursor.col < line_len {
                self.cursor.col += 1;
            }
        }
    }

    /// vim `r<c>`: replace the character under the cursor with `new_c`.
    /// No-op if the cursor sits on a newline or past end of buffer.
    pub fn replace_char(&mut self, new_c: char) {
        self.transform_char_at_cursor(|_| new_c);
    }

    pub fn uppercase_selection(&mut self) {
        self.map_selection_case(true);
    }

    pub fn lowercase_selection(&mut self) {
        self.map_selection_case(false);
    }

    // str::to_uppercase/to_lowercase (not byte ASCII) so length-growing folds like ß→SS survive; cursor pins to the selection start for vim U/u parity.
    fn map_selection_case(&mut self, upper: bool) {
        let Some((start, end)) = self.selection_range() else {
            return;
        };
        let source = self.rope.slice(start..end).to_string();
        let mapped = if upper {
            source.to_uppercase()
        } else {
            source.to_lowercase()
        };
        self.replace_selection_with_str(&mapped);
        self.set_cursor_from_char_index(start);
    }

    /// Apply `f` to the char at the cursor and write the result back. Returns
    /// `false` when there is nothing to transform (cursor past end / on
    /// newline) so callers can short-circuit any cursor advance.
    fn transform_char_at_cursor(&mut self, f: impl FnOnce(char) -> char) -> bool {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let idx = self.char_index();
        if idx >= len {
            return false;
        }
        let current = self.rope.char(idx);
        if current == '\n' {
            return false;
        }
        let next = f(current);
        if next == current {
            return true;
        }
        let before = self.cursor;
        self.begin_undo_group();
        let mut prev_buf = [0u8; 4];
        let prev_s = current.encode_utf8(&mut prev_buf).to_string();
        self.record_delete(idx, idx + 1);
        self.rope.remove(idx..idx + 1);
        self.record_delete_delta(
            idx,
            &prev_s,
            before,
            super::super::undo_log::GroupHint::Explicit,
        );
        let mut buf = [0u8; 4];
        let s = next.encode_utf8(&mut buf);
        self.record_insert(idx, s);
        self.rope.insert(idx, s);
        self.record_insert_delta(idx, s, before, super::super::undo_log::GroupHint::Explicit);
        self.dirty = true;
        self.end_undo_group();
        true
    }
}

fn toggle_ascii_case(c: char) -> char {
    if c.is_ascii_uppercase() {
        c.to_ascii_lowercase()
    } else if c.is_ascii_lowercase() {
        c.to_ascii_uppercase()
    } else {
        c
    }
}

fn count_leading_blanks(rope: &ropey::Rope, start: usize, len: usize) -> usize {
    let mut n = 0;
    while n < len {
        let c = rope.char(start + n);
        if c != ' ' && c != '\t' {
            break;
        }
        n += 1;
    }
    n
}
