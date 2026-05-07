use super::super::Buffer;

impl Buffer {
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
            self.replace_char_at(idx, new_c);
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
        self.replace_char_at(idx, new_c);
    }

    fn replace_char_at(&mut self, idx: usize, new_c: char) {
        self.record_delete(idx, idx + 1);
        self.rope.remove(idx..idx + 1);
        let mut buf = [0u8; 4];
        let s = new_c.encode_utf8(&mut buf);
        self.record_insert(idx, s);
        self.rope.insert(idx, s);
        self.dirty = true;
    }
}
