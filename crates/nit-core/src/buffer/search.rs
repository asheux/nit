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
        self.search_line_matches_opt(line, term, whole_word, false)
    }

    /// Variant of `search_line_matches` with vim-style case folding. When
    /// `case_insensitive`, both `term` and the buffer slice are lowercased
    /// before comparing — implements the case-folded side of smart-case.
    pub fn search_line_matches_opt(
        &self,
        line: usize,
        term: &str,
        whole_word: bool,
        case_insensitive: bool,
    ) -> Vec<(usize, usize)> {
        let mut hits = Vec::new();
        if term.is_empty() || line >= self.rope.len_lines() {
            return hits;
        }
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let term_chars: Vec<char> = if case_insensitive {
            term.chars().flat_map(char::to_lowercase).collect()
        } else {
            term.chars().collect()
        };
        let term_len = term_chars.len();
        if term_len == 0 || term_len > line_len {
            return hits;
        }
        let raw_line_chars: Vec<char> = self
            .rope
            .slice(line_start..line_start + line_len)
            .chars()
            .collect();
        let line_chars: Vec<char> = if case_insensitive {
            raw_line_chars
                .iter()
                .flat_map(|c| c.to_lowercase())
                .collect()
        } else {
            raw_line_chars.clone()
        };
        if line_chars.len() != raw_line_chars.len() {
            return self.fallback_case_fold_matches(line, term, whole_word);
        }
        let mut at = 0;
        while at + term_len <= line_len {
            let end = at + term_len;
            let chars_match = line_chars[at..end] == term_chars[..];
            let boundary_ok = !whole_word
                || ((at == 0 || !is_word_char(raw_line_chars[at - 1]))
                    && (end >= line_len || !is_word_char(raw_line_chars[end])));
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

    /// Conservative O(n*m) scan used when Unicode case folding expands chars
    /// (Turkish `İ` → `i̇`, etc.) and the column indices would otherwise
    /// drift. Rare path; keeps highlight correctness without a Unicode-aware
    /// indexing layer.
    fn fallback_case_fold_matches(
        &self,
        line: usize,
        term: &str,
        whole_word: bool,
    ) -> Vec<(usize, usize)> {
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let raw_line_chars: Vec<char> = self
            .rope
            .slice(line_start..line_start + line_len)
            .chars()
            .collect();
        let term_lower: String = term.chars().flat_map(char::to_lowercase).collect();
        let term_count = term.chars().count();
        let mut hits = Vec::new();
        let mut at = 0;
        while at + term_count <= raw_line_chars.len() {
            let candidate: String = raw_line_chars[at..at + term_count]
                .iter()
                .flat_map(|c| c.to_lowercase())
                .collect();
            let chars_match = candidate == term_lower;
            let end = at + term_count;
            let boundary_ok = !whole_word
                || ((at == 0 || !is_word_char(raw_line_chars[at - 1]))
                    && (end >= raw_line_chars.len() || !is_word_char(raw_line_chars[end])));
            if chars_match && boundary_ok {
                hits.push((at, end));
                at = end;
            } else {
                at += 1;
            }
        }
        hits
    }

    /// Move cursor to next match of `term` after the cursor; wrap to top on miss.
    pub fn search_next_match(&mut self, term: &str, whole_word: bool) -> bool {
        self.search_in_direction(term, whole_word, false, true)
    }

    /// Move cursor to previous match before cursor; wrap to bottom on miss.
    /// If cursor is inside a match, the boundary is that match's start (so we
    /// skip past the enclosing occurrence rather than snapping to its start).
    pub fn search_prev_match(&mut self, term: &str, whole_word: bool) -> bool {
        self.search_in_direction(term, whole_word, false, false)
    }

    pub fn search_next_match_opt(
        &mut self,
        term: &str,
        whole_word: bool,
        case_insensitive: bool,
    ) -> bool {
        self.search_in_direction(term, whole_word, case_insensitive, true)
    }

    pub fn search_prev_match_opt(
        &mut self,
        term: &str,
        whole_word: bool,
        case_insensitive: bool,
    ) -> bool {
        self.search_in_direction(term, whole_word, case_insensitive, false)
    }

    /// Place the cursor at the first match at-or-after `(start_line, start_col)`,
    /// wrapping to the buffer head if nothing is found between the seed and
    /// EOF. Used by `/` incremental search so every keystroke re-runs from the
    /// position the prompt opened at — preventing the cursor from "drifting"
    /// across matches as the user types.
    pub fn search_seek_first_match(
        &mut self,
        term: &str,
        whole_word: bool,
        case_insensitive: bool,
        start_line: usize,
        start_col: usize,
    ) -> bool {
        if term.is_empty() {
            return false;
        }
        let total_lines = self.rope.len_lines();
        if total_lines == 0 {
            return false;
        }
        let same_line =
            self.search_line_matches_opt(start_line, term, whole_word, case_insensitive);
        if let Some((col, _)) = same_line.iter().find(|(s, _)| *s >= start_col) {
            self.cursor.line = start_line;
            self.cursor.col = *col;
            self.clamp_col();
            return true;
        }
        for offset in 1..=total_lines {
            let line = (start_line + offset) % total_lines;
            let hits = self.search_line_matches_opt(line, term, whole_word, case_insensitive);
            if let Some((col, _)) = hits.first() {
                self.cursor.line = line;
                self.cursor.col = *col;
                self.clamp_col();
                return true;
            }
            if line == start_line {
                break;
            }
        }
        false
    }

    fn search_in_direction(
        &mut self,
        term: &str,
        whole_word: bool,
        case_insensitive: bool,
        forward: bool,
    ) -> bool {
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

        let same_line =
            self.search_line_matches_opt(cursor_line, term, whole_word, case_insensitive);
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
            let line_hits = self.search_line_matches_opt(l, term, whole_word, case_insensitive);
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
