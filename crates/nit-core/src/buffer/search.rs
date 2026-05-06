use super::cursor_motion::is_word_char;
use super::Buffer;

impl Buffer {
    /// vim `f` / `F` / `t` / `T`: search for `ch` on the current line only.
    /// Returns `true` if the character was found and the cursor moved.
    pub fn find_char_in_line(&mut self, ch: char, forward: bool, till: bool) -> bool {
        self.end_edit_group();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let cur_col = self.cursor.col.min(line_len);
        if forward {
            let start = cur_col.saturating_add(1);
            let mut col = start;
            while col < line_len {
                if self.rope.char(line_start + col) == ch {
                    let target = if till { col.saturating_sub(1) } else { col };
                    if till && target < cur_col {
                        return false;
                    }
                    self.cursor.col = target;
                    return true;
                }
                col += 1;
            }
        } else {
            if cur_col == 0 {
                return false;
            }
            let mut col = cur_col - 1;
            loop {
                if self.rope.char(line_start + col) == ch {
                    let target = if till { col + 1 } else { col };
                    if till && target > cur_col {
                        return false;
                    }
                    self.cursor.col = target;
                    return true;
                }
                if col == 0 {
                    break;
                }
                col -= 1;
            }
        }
        false
    }

    /// Identifier-style word under the cursor (alnum + `_`); scans forward on
    /// the current line if the cursor is not on a word char.
    pub fn word_at_cursor(&self) -> Option<String> {
        let len = self.rope.len_chars();
        if len == 0 {
            return None;
        }
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let mut col = self.cursor.col.min(line_len);

        if col >= line_len || !is_word_char(self.rope.char(line_start + col)) {
            while col < line_len && !is_word_char(self.rope.char(line_start + col)) {
                col += 1;
            }
            if col >= line_len {
                return None;
            }
        }

        let mut start = col;
        while start > 0 && is_word_char(self.rope.char(line_start + start - 1)) {
            start -= 1;
        }
        let mut end = col;
        while end < line_len && is_word_char(self.rope.char(line_start + end)) {
            end += 1;
        }
        if end <= start {
            return None;
        }
        Some(
            self.rope
                .slice(line_start + start..line_start + end)
                .to_string(),
        )
    }

    /// Character-index ranges on `line` that match `term` as `(col_start, col_end)`.
    /// `whole_word` requires non-word boundaries (or line edges) on both sides.
    pub fn search_line_matches(
        &self,
        line: usize,
        term: &str,
        whole_word: bool,
    ) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if term.is_empty() || line >= self.rope.len_lines() {
            return out;
        }
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        if line_len == 0 {
            return out;
        }
        let term_chars: Vec<char> = term.chars().collect();
        let tlen = term_chars.len();
        if tlen == 0 || tlen > line_len {
            return out;
        }
        let mut i = 0;
        while i + tlen <= line_len {
            let mut matched = true;
            for (k, &tc) in term_chars.iter().enumerate() {
                if self.rope.char(line_start + i + k) != tc {
                    matched = false;
                    break;
                }
            }
            if matched {
                if whole_word {
                    let before_ok = i == 0 || !is_word_char(self.rope.char(line_start + i - 1));
                    let after_ok = i + tlen >= line_len
                        || !is_word_char(self.rope.char(line_start + i + tlen));
                    if before_ok && after_ok {
                        out.push((i, i + tlen));
                        i += tlen;
                        continue;
                    }
                } else {
                    out.push((i, i + tlen));
                    i += tlen;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// Move cursor to next match of `term` after the cursor; wrap to top on miss.
    pub fn search_next_match(&mut self, term: &str, whole_word: bool) -> bool {
        if term.is_empty() {
            return false;
        }
        self.end_edit_group();
        let total_lines = self.rope.len_lines();
        if total_lines == 0 {
            return false;
        }
        let cursor_line = self.cursor.line;
        let cursor_col = self.cursor.col;
        for (s, _e) in self.search_line_matches(cursor_line, term, whole_word) {
            if s > cursor_col {
                self.cursor.line = cursor_line;
                self.cursor.col = s;
                self.clamp_col();
                return true;
            }
        }
        for l in (cursor_line + 1)..total_lines {
            let matches = self.search_line_matches(l, term, whole_word);
            if let Some(&(s, _)) = matches.first() {
                self.cursor.line = l;
                self.cursor.col = s;
                self.clamp_col();
                return true;
            }
        }
        // Wrap; on cursor_line accept first match regardless of position.
        for l in 0..=cursor_line {
            let matches = self.search_line_matches(l, term, whole_word);
            if let Some(&(s, _)) = matches.first() {
                self.cursor.line = l;
                self.cursor.col = s;
                self.clamp_col();
                return true;
            }
        }
        false
    }

    /// Move cursor to previous match before cursor; wrap to bottom on miss.
    /// If cursor is inside a match, the boundary is that match's start (so we
    /// skip past the enclosing occurrence rather than snapping to its start).
    pub fn search_prev_match(&mut self, term: &str, whole_word: bool) -> bool {
        if term.is_empty() {
            return false;
        }
        self.end_edit_group();
        let total_lines = self.rope.len_lines();
        if total_lines == 0 {
            return false;
        }
        let cursor_line = self.cursor.line;
        let cursor_col = self.cursor.col;
        let matches = self.search_line_matches(cursor_line, term, whole_word);
        let enclosing_start = matches
            .iter()
            .find(|(s, e)| *s <= cursor_col && *e > cursor_col)
            .map(|(s, _)| *s);
        let boundary = enclosing_start.unwrap_or(cursor_col);
        if let Some(&(s, _)) = matches.iter().rev().find(|(s, _)| *s < boundary) {
            self.cursor.line = cursor_line;
            self.cursor.col = s;
            self.clamp_col();
            return true;
        }
        for l in (0..cursor_line).rev() {
            let matches = self.search_line_matches(l, term, whole_word);
            if let Some(&(s, _)) = matches.last() {
                self.cursor.line = l;
                self.cursor.col = s;
                self.clamp_col();
                return true;
            }
        }
        for l in (cursor_line..total_lines).rev() {
            let matches = self.search_line_matches(l, term, whole_word);
            if let Some(&(s, _)) = matches.last() {
                self.cursor.line = l;
                self.cursor.col = s;
                self.clamp_col();
                return true;
            }
        }
        false
    }
}
