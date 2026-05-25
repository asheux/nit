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

    /// vim `e`: end of current/next "word" — three-class transitions where
    /// `Word`, `Punct`, and `Whitespace` are each treated as separate runs.
    pub fn move_word_end(&mut self) {
        self.end_edit_group();
        if let Some(idx) = self.scan_word_end_forward() {
            self.set_cursor_from_char_index(idx);
        }
    }

    /// vim `b`: previous "word" start using three-class transitions.
    pub fn move_word_back(&mut self) {
        self.end_edit_group();
        let idx = self.scan_word_start_back();
        self.set_cursor_from_char_index(idx);
    }

    /// vim `w`: start of next "word" run (word OR punct), skipping
    /// whitespace between runs.
    pub fn move_word_forward(&mut self) {
        self.end_edit_group();
        if let Some(idx) = self.scan_word_start_forward() {
            self.set_cursor_from_char_index(idx);
        }
    }

    /// vim `W`: start of next WORD (whitespace-separated).
    pub fn move_big_word_forward(&mut self) {
        self.end_edit_group();
        if let Some(idx) = self.scan_big_word_start_forward() {
            self.set_cursor_from_char_index(idx);
        }
    }

    /// vim `B`: previous WORD start.
    pub fn move_big_word_back(&mut self) {
        self.end_edit_group();
        let idx = self.scan_big_word_start_back();
        self.set_cursor_from_char_index(idx);
    }

    /// vim `E`: end of current/next WORD.
    pub fn move_big_word_end(&mut self) {
        self.end_edit_group();
        if let Some(idx) = self.scan_big_word_end_forward() {
            self.set_cursor_from_char_index(idx);
        }
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

    /// Position the cursor would land on after `w`, or `None` if the cursor
    /// is already at the end of buffer. Shared between `move_word_forward`
    /// and `delete_word_forward` so a `dw` deletes exactly the span `w`
    /// would traverse.
    pub(super) fn scan_word_start_forward(&self) -> Option<usize> {
        let len = self.rope.len_chars();
        if len == 0 {
            return None;
        }
        let mut idx = self.char_index().min(len);
        if idx >= len {
            return None;
        }
        let cls = char_class(self.rope.char(idx));
        if cls != CharClass::Whitespace {
            while idx < len && char_class(self.rope.char(idx)) == cls {
                idx += 1;
            }
        }
        while idx < len && self.rope.char(idx).is_whitespace() {
            idx += 1;
        }
        if idx >= len {
            idx = len - 1;
        }
        Some(idx)
    }

    /// Position the cursor would land on after `b`. Walks back past any
    /// trailing whitespace then to the start of whichever class lies
    /// behind the cursor.
    pub(super) fn scan_word_start_back(&self) -> usize {
        let len = self.rope.len_chars();
        if len == 0 {
            return 0;
        }
        let mut idx = self.char_index().min(len);
        if idx == 0 {
            return 0;
        }
        if idx >= len {
            idx = len.saturating_sub(1);
        }
        // Step back one so the cursor leaves its current class boundary.
        idx -= 1;
        while idx > 0 && self.rope.char(idx).is_whitespace() {
            idx -= 1;
        }
        if !self.rope.char(idx).is_whitespace() {
            let cls = char_class(self.rope.char(idx));
            while idx > 0 && char_class(self.rope.char(idx - 1)) == cls {
                idx -= 1;
            }
        }
        idx
    }

    /// Position the cursor would land on after `e`. Advances past the
    /// current class if the cursor sits on its trailing edge so repeated
    /// `e` presses walk forward instead of stalling on a boundary.
    pub(super) fn scan_word_end_forward(&self) -> Option<usize> {
        let len = self.rope.len_chars();
        if len == 0 {
            return None;
        }
        let mut idx = self.char_index().min(len);
        if idx >= len {
            return None;
        }
        let cur_cls = char_class(self.rope.char(idx));
        let at_class_edge = cur_cls == CharClass::Whitespace
            || idx + 1 >= len
            || char_class(self.rope.char(idx + 1)) != cur_cls;
        if at_class_edge {
            idx += 1;
            while idx < len && self.rope.char(idx).is_whitespace() {
                idx += 1;
            }
            if idx >= len {
                return Some(len - 1);
            }
        }
        let cls = char_class(self.rope.char(idx));
        while idx + 1 < len && char_class(self.rope.char(idx + 1)) == cls {
            idx += 1;
        }
        Some(idx)
    }

    /// `W`-equivalent landing point: any non-whitespace run is one big WORD.
    pub(super) fn scan_big_word_start_forward(&self) -> Option<usize> {
        let len = self.rope.len_chars();
        if len == 0 {
            return None;
        }
        let mut idx = self.char_index().min(len);
        if idx >= len {
            return None;
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
        Some(idx)
    }

    /// `B`-equivalent landing point: walks back past whitespace then the
    /// entire previous non-whitespace run.
    pub(super) fn scan_big_word_start_back(&self) -> usize {
        let len = self.rope.len_chars();
        if len == 0 {
            return 0;
        }
        let mut idx = self.char_index().min(len);
        if idx == 0 {
            return 0;
        }
        if idx >= len {
            idx = len.saturating_sub(1);
        }
        idx -= 1;
        while idx > 0 && self.rope.char(idx).is_whitespace() {
            idx -= 1;
        }
        if !self.rope.char(idx).is_whitespace() {
            while idx > 0 && !self.rope.char(idx - 1).is_whitespace() {
                idx -= 1;
            }
        }
        idx
    }

    /// `E`-equivalent landing point: end of current/next non-whitespace run.
    pub(super) fn scan_big_word_end_forward(&self) -> Option<usize> {
        let len = self.rope.len_chars();
        if len == 0 {
            return None;
        }
        let mut idx = self.char_index().min(len);
        if idx >= len {
            return None;
        }
        let on_nonws = !self.rope.char(idx).is_whitespace();
        let at_word_end = on_nonws && (idx + 1 >= len || self.rope.char(idx + 1).is_whitespace());
        if at_word_end || !on_nonws {
            idx += 1;
            while idx < len && self.rope.char(idx).is_whitespace() {
                idx += 1;
            }
            if idx >= len {
                return Some(len - 1);
            }
        }
        while idx + 1 < len && !self.rope.char(idx + 1).is_whitespace() {
            idx += 1;
        }
        Some(idx)
    }
}

/// Three-class character partition used by every vim-style word motion.
/// Word characters are alphanumerics plus underscore (vim's default
/// `iskeyword`-relaxed view); punctuation is everything printable that
/// isn't whitespace or a word char. Distinguishing punct from whitespace
/// is what makes `w` land on `"` in `foo "bar"` instead of skipping
/// straight to `bar`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum CharClass {
    Whitespace,
    Word,
    Punct,
}

pub(super) fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else if is_word_char(c) {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

pub(super) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
