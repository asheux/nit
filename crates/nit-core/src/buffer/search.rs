use super::cursor_motion::is_word_char;
use super::Buffer;

impl Buffer {
    /// vim `f` / `F` / `t` / `T`: search for `ch` on the current line only.
    /// Returns `true` if the character was found and the cursor moved.
    pub fn find_char_in_line(&mut self, ch: char, forward: bool, till: bool) -> bool {
        self.end_edit_group();
        let line = self.clamped_cursor_line();
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let cur_col = self.cursor.col.min(line_len);

        let landing = if forward {
            let probe = cur_col.saturating_add(1);
            (probe..line_len).find(|&col| self.rope.char(line_start + col) == ch)
        } else if cur_col == 0 {
            None
        } else {
            (0..cur_col)
                .rev()
                .find(|&col| self.rope.char(line_start + col) == ch)
        };
        let Some(landing) = landing else {
            return false;
        };

        let target = match (forward, till) {
            (true, true) => landing.saturating_sub(1),
            (false, true) => landing + 1,
            _ => landing,
        };
        // till must not snap onto-or-past the cursor (vim disallows zero/backward t-jumps).
        if till && ((forward && target < cur_col) || (!forward && target > cur_col)) {
            return false;
        }
        self.cursor.col = target;
        true
    }

    /// Identifier-style word under the cursor (alnum + `_`); scans forward on
    /// the current line if the cursor is not on a word char.
    pub fn word_at_cursor(&self) -> Option<String> {
        if self.rope.len_chars() == 0 {
            return None;
        }
        let line = self.clamped_cursor_line();
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let initial = self.cursor.col.min(line_len);
        let pivot = (initial..line_len).find(|&i| is_word_char(self.rope.char(line_start + i)))?;

        let mut start = pivot;
        while start > 0 && is_word_char(self.rope.char(line_start + start - 1)) {
            start -= 1;
        }
        let mut end = pivot;
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
        let mut hits = Vec::new();
        if term.is_empty() || line >= self.rope.len_lines() {
            return hits;
        }
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let term_chars: Vec<char> = term.chars().collect();
        let term_len = term_chars.len();
        if term_len == 0 || term_len > line_len {
            return hits;
        }
        // Materialize once: per-position rope reads traverse the tree on each call.
        let line_chars: Vec<char> = self
            .rope
            .slice(line_start..line_start + line_len)
            .chars()
            .collect();
        let mut at = 0;
        while at + term_len <= line_len {
            let end = at + term_len;
            let chars_match = line_chars[at..end] == term_chars[..];
            let boundary_ok = !whole_word
                || ((at == 0 || !is_word_char(line_chars[at - 1]))
                    && (end >= line_len || !is_word_char(line_chars[end])));
            let stride = if chars_match && boundary_ok {
                hits.push((at, end));
                term_len
            } else {
                1
            };
            at += stride;
        }
        hits
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

        // Pass 1: same line as cursor. Forward picks the first match strictly
        // past the cursor; backward steps off the enclosing match (if any) so
        // the cursor doesn't snap onto its current match's start.
        let same_line = self.search_line_matches(cursor_line, term, whole_word);
        let pass_one = if forward {
            same_line
                .iter()
                .find(|(start, _)| *start > cursor_col)
                .map(|(start, _)| *start)
        } else {
            let enclosing = same_line
                .iter()
                .find(|(start, end)| *start <= cursor_col && *end > cursor_col)
                .map(|(start, _)| *start);
            let boundary = enclosing.unwrap_or(cursor_col);
            same_line
                .iter()
                .rev()
                .find(|(start, _)| *start < boundary)
                .map(|(start, _)| *start)
        };
        if let Some(col) = pass_one {
            self.cursor.line = cursor_line;
            self.cursor.col = col;
            self.clamp_col();
            return true;
        }

        // Passes 2 + 3: walk past the cursor in `forward` direction, then wrap
        // around to the cursor's own line so prior occurrences are reachable.
        let mut probe: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new(((cursor_line + 1)..total_lines).chain(0..=cursor_line))
        } else {
            Box::new(
                (0..cursor_line)
                    .rev()
                    .chain((cursor_line..total_lines).rev()),
            )
        };
        let hit = probe.find_map(|l| {
            let line_hits = self.search_line_matches(l, term, whole_word);
            let pick = if forward {
                line_hits.first()
            } else {
                line_hits.last()
            };
            pick.map(|(start, _)| (l, *start))
        });
        let Some((line, col)) = hit else {
            return false;
        };
        self.cursor.line = line;
        self.cursor.col = col;
        self.clamp_col();
        true
    }
}
