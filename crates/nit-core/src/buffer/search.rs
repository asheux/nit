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
        let term_chars: Vec<char> = term.chars().collect();
        let tlen = term_chars.len();
        if tlen == 0 || tlen > line_len {
            return out;
        }
        // Materialize the line slice once so the inner match avoids per-position
        // rope reads (which traverse the tree on each call).
        let line_chars: Vec<char> = self
            .rope
            .slice(line_start..line_start + line_len)
            .chars()
            .collect();
        let mut i = 0;
        while i + tlen <= line_len {
            if line_chars[i..i + tlen] != term_chars[..] {
                i += 1;
                continue;
            }
            if whole_word && !word_boundaries_ok(&line_chars, i, tlen, line_len) {
                i += 1;
                continue;
            }
            out.push((i, i + tlen));
            i += tlen;
        }
        out
    }

    /// Move cursor to next match of `term` after the cursor; wrap to top on miss.
    pub fn search_next_match(&mut self, term: &str, whole_word: bool) -> bool {
        self.search_in_direction(term, whole_word, true)
    }

    /// Move cursor to previous match before cursor; wrap to bottom on miss.
    /// If cursor is inside a match, the boundary is that match's start (so we
    /// skip past the enclosing occurrence rather than snapping to its start).
    pub fn search_prev_match(&mut self, term: &str, whole_word: bool) -> bool {
        self.search_in_direction(term, whole_word, false)
    }

    fn search_in_direction(&mut self, term: &str, whole_word: bool, forward: bool) -> bool {
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

        // Pass 1: same line as cursor, after/before the cursor column.
        if let Some(s) = cursor_line_match(self, cursor_line, cursor_col, term, whole_word, forward)
        {
            return self.move_to_match(cursor_line, s);
        }

        // Pass 2: lines past the cursor in the chosen direction.
        let beyond: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new((cursor_line + 1)..total_lines)
        } else {
            Box::new((0..cursor_line).rev())
        };
        for l in beyond {
            if let Some(s) = first_or_last_match(self, l, term, whole_word, forward) {
                return self.move_to_match(l, s);
            }
        }

        // Pass 3: wrap. Forward wraps from top through cursor_line; backward
        // wraps from bottom back through cursor_line.
        let wrap: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new(0..=cursor_line)
        } else {
            Box::new((cursor_line..total_lines).rev())
        };
        for l in wrap {
            if let Some(s) = first_or_last_match(self, l, term, whole_word, forward) {
                return self.move_to_match(l, s);
            }
        }
        false
    }

    fn move_to_match(&mut self, line: usize, col: usize) -> bool {
        self.cursor.line = line;
        self.cursor.col = col;
        self.clamp_col();
        true
    }
}

fn word_boundaries_ok(line_chars: &[char], i: usize, tlen: usize, line_len: usize) -> bool {
    let before_ok = i == 0 || !is_word_char(line_chars[i - 1]);
    let after_ok = i + tlen >= line_len || !is_word_char(line_chars[i + tlen]);
    before_ok && after_ok
}

fn cursor_line_match(
    buffer: &Buffer,
    line: usize,
    cursor_col: usize,
    term: &str,
    whole_word: bool,
    forward: bool,
) -> Option<usize> {
    let matches = buffer.search_line_matches(line, term, whole_word);
    if forward {
        matches
            .iter()
            .find(|(s, _)| *s > cursor_col)
            .map(|(s, _)| *s)
    } else {
        let enclosing_start = matches
            .iter()
            .find(|(s, e)| *s <= cursor_col && *e > cursor_col)
            .map(|(s, _)| *s);
        let boundary = enclosing_start.unwrap_or(cursor_col);
        matches
            .iter()
            .rev()
            .find(|(s, _)| *s < boundary)
            .map(|(s, _)| *s)
    }
}

fn first_or_last_match(
    buffer: &Buffer,
    line: usize,
    term: &str,
    whole_word: bool,
    forward: bool,
) -> Option<usize> {
    let matches = buffer.search_line_matches(line, term, whole_word);
    if forward {
        matches.first().map(|(s, _)| *s)
    } else {
        matches.last().map(|(s, _)| *s)
    }
}
