use super::Buffer;

impl Buffer {
    pub fn move_left(&mut self) {
        self.end_edit_group();
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.line_char_len(self.cursor.line);
        }
    }

    pub fn move_right(&mut self) {
        self.end_edit_group();
        let len = self.line_char_len(self.cursor.line);
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.rope.len_lines() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        self.end_edit_group();
        if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.clamp_col();
        }
    }

    pub fn move_down(&mut self) {
        self.end_edit_group();
        if self.cursor.line + 1 < self.rope.len_lines() {
            self.cursor.line += 1;
            self.clamp_col();
        }
    }

    pub fn page_up(&mut self, count: usize) {
        self.end_edit_group();
        self.cursor.line -= count.min(self.cursor.line);
        self.clamp_col();
    }

    pub fn page_down(&mut self, count: usize) {
        self.end_edit_group();
        let max_line = self.rope.len_lines().saturating_sub(1);
        self.cursor.line = (self.cursor.line + count).min(max_line);
        self.clamp_col();
    }

    pub fn move_home(&mut self) {
        self.end_edit_group();
        self.cursor.col = 0;
    }

    pub fn move_end(&mut self) {
        self.end_edit_group();
        self.cursor.col = self.line_char_len(self.cursor.line);
    }

    pub fn append(&mut self) {
        self.end_edit_group();
        let len = self.line_char_len(self.cursor.line);
        if self.cursor.col < len {
            self.cursor.col += 1;
        }
    }

    pub fn exit_insert_mode(&mut self) {
        self.end_edit_group();
        if self.is_line_blank(self.cursor.line) {
            self.cursor.col = 0;
        } else if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
    }

    pub fn go_to_top(&mut self) {
        self.end_edit_group();
        self.cursor.line = 0;
        self.clamp_col();
    }

    pub fn go_to_bottom(&mut self) {
        self.end_edit_group();
        self.cursor.line = self.rope.len_lines().saturating_sub(1);
        self.clamp_col();
    }

    /// Jump to a specific line number, 1-indexed. Clamps to the buffer's
    /// last line if the requested number exceeds it; clamps to line 1 if
    /// the request is 0. Drives both `:N` command and `NG` motion.
    pub fn go_to_line(&mut self, line_one_indexed: usize) {
        self.end_edit_group();
        let last = self.rope.len_lines().saturating_sub(1);
        let target = line_one_indexed.saturating_sub(1).min(last);
        self.cursor.line = target;
        self.clamp_col();
    }

    /// vim `e`: end of current/next "word" (alnum + `_`).
    pub fn move_word_end(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index();
        if idx >= len {
            return;
        }
        // Already on the last char of a word — jump past it so `e` advances.
        if is_word_char(self.rope.char(idx))
            && idx + 1 < len
            && !is_word_char(self.rope.char(idx + 1))
        {
            idx += 1;
        }
        while idx < len && !is_word_char(self.rope.char(idx)) {
            idx += 1;
        }
        if idx >= len {
            return;
        }
        while idx + 1 < len && is_word_char(self.rope.char(idx + 1)) {
            idx += 1;
        }
        self.set_cursor_from_char_index(idx);
    }

    /// vim `b`: previous "word" start.
    pub fn move_word_back(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index();
        if idx == 0 {
            return;
        }
        if idx >= len {
            idx = len - 1;
        }
        if is_word_char(self.rope.char(idx)) {
            if idx > 0 && !is_word_char(self.rope.char(idx - 1)) {
                idx -= 1;
            }
        } else {
            idx -= 1;
        }
        while idx > 0 && !is_word_char(self.rope.char(idx)) {
            idx -= 1;
        }
        if !is_word_char(self.rope.char(idx)) {
            return;
        }
        while idx > 0 && is_word_char(self.rope.char(idx - 1)) {
            idx -= 1;
        }
        self.set_cursor_from_char_index(idx);
    }

    /// vim `w`: start of next "word" (alnum + `_`) OR next run of
    /// non-whitespace punctuation. Whitespace is skipped.
    pub fn move_word_forward(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index().min(len);
        if idx >= len {
            return;
        }
        let cur = self.rope.char(idx);
        if cur.is_whitespace() {
            // skip leading whitespace below
        } else if is_word_char(cur) {
            while idx < len && is_word_char(self.rope.char(idx)) {
                idx += 1;
            }
        } else {
            while idx < len {
                let c = self.rope.char(idx);
                if c.is_whitespace() || is_word_char(c) {
                    break;
                }
                idx += 1;
            }
        }
        while idx < len && self.rope.char(idx).is_whitespace() {
            idx += 1;
        }
        if idx >= len {
            idx = len - 1;
        }
        self.set_cursor_from_char_index(idx);
    }

    /// vim `W`: start of next WORD (whitespace-separated).
    pub fn move_big_word_forward(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index().min(len);
        if idx >= len {
            return;
        }
        while idx < len && !self.rope.char(idx).is_whitespace() {
            idx += 1;
        }
        while idx < len && self.rope.char(idx).is_whitespace() {
            idx += 1;
        }
        if idx >= len {
            idx = len - 1;
        }
        self.set_cursor_from_char_index(idx);
    }

    /// vim `B`: previous WORD start.
    pub fn move_big_word_back(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index().min(len);
        if idx == 0 {
            return;
        }
        idx -= 1;
        while idx > 0 && self.rope.char(idx).is_whitespace() {
            idx -= 1;
        }
        if self.rope.char(idx).is_whitespace() {
            self.set_cursor_from_char_index(idx);
            return;
        }
        while idx > 0 && !self.rope.char(idx - 1).is_whitespace() {
            idx -= 1;
        }
        self.set_cursor_from_char_index(idx);
    }

    /// vim `E`: end of current/next WORD.
    pub fn move_big_word_end(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index().min(len);
        if idx + 1 >= len {
            if idx < len {
                self.set_cursor_from_char_index(len - 1);
            }
            return;
        }
        let on_nonws = !self.rope.char(idx).is_whitespace();
        let at_word_end = on_nonws && self.rope.char(idx + 1).is_whitespace();
        if at_word_end || self.rope.char(idx).is_whitespace() {
            idx += 1;
            while idx < len && self.rope.char(idx).is_whitespace() {
                idx += 1;
            }
        }
        if idx >= len {
            return;
        }
        while idx + 1 < len && !self.rope.char(idx + 1).is_whitespace() {
            idx += 1;
        }
        self.set_cursor_from_char_index(idx);
    }

    /// vim `^`: first non-blank character on the line.
    pub fn move_first_non_blank(&mut self) {
        self.end_edit_group();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let mut col = 0;
        while col < line_len {
            let c = self.rope.char(line_start + col);
            if c != ' ' && c != '\t' {
                break;
            }
            col += 1;
        }
        self.cursor.col = col;
    }

    /// vim `g_`: last non-blank character on the line.
    pub fn move_last_non_blank(&mut self) {
        self.end_edit_group();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        if line_len == 0 {
            self.cursor.col = 0;
            return;
        }
        let mut col = line_len - 1;
        loop {
            let c = self.rope.char(line_start + col);
            if c != ' ' && c != '\t' {
                break;
            }
            if col == 0 {
                break;
            }
            col -= 1;
        }
        self.cursor.col = col;
    }

    /// vim `{`: previous blank-line paragraph boundary.
    pub fn move_paragraph_up(&mut self) {
        self.end_edit_group();
        if self.cursor.line == 0 {
            self.cursor.col = 0;
            return;
        }
        let mut line = self.cursor.line - 1;
        while line > 0 && self.is_line_blank(line) {
            line -= 1;
        }
        while line > 0 && !self.is_line_blank(line) {
            line -= 1;
        }
        self.cursor.line = line;
        self.cursor.col = 0;
    }

    /// vim `}`: next blank-line paragraph boundary.
    pub fn move_paragraph_down(&mut self) {
        self.end_edit_group();
        let total = self.rope.len_lines();
        if total == 0 {
            return;
        }
        let last = total - 1;
        let mut line = (self.cursor.line + 1).min(last);
        while line < last && self.is_line_blank(line) {
            line += 1;
        }
        while line < last && !self.is_line_blank(line) {
            line += 1;
        }
        self.cursor.line = line;
        self.cursor.col = 0;
        self.clamp_col();
    }

    /// vim `H` / `M` / `L`: jump cursor to a row offset within the visible viewport.
    pub fn move_viewport_top(&mut self) {
        self.end_edit_group();
        let last = self.rope.len_lines().saturating_sub(1);
        self.cursor.line = self.viewport.offset_line.min(last);
        self.clamp_col();
    }

    pub fn move_viewport_middle(&mut self) {
        self.end_edit_group();
        let last = self.rope.len_lines().saturating_sub(1);
        let h = self.viewport.height.max(1);
        self.cursor.line = (self.viewport.offset_line + h / 2).min(last);
        self.clamp_col();
    }

    pub fn move_viewport_bottom(&mut self) {
        self.end_edit_group();
        let last = self.rope.len_lines().saturating_sub(1);
        let h = self.viewport.height.max(1);
        self.cursor.line = (self.viewport.offset_line + h.saturating_sub(1)).min(last);
        self.clamp_col();
    }
}

pub(super) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
