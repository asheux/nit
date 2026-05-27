use super::Buffer;

/// Cap on the lines scanned to infer the buffer's indent unit. Bounds the
/// inference cost on huge files; the first ~200 indented lines are nearly
/// always representative.
const INDENT_SCAN_LINES: usize = 200;

/// Minimum/maximum widths accepted by [`Buffer::indent_unit`]. A 1-space unit
/// is honored only when the gcd of observed widths actually equals 1 (e.g. an
/// off-by-one mis-indent); >8 is treated as noise and clamped down.
const MIN_INDENT_WIDTH: usize = 1;
const MAX_INDENT_WIDTH: usize = 8;

const FALLBACK_INDENT: &str = "    ";

impl Buffer {
    /// Indent every line touched by the active selection (or the current
    /// line when no selection is set) by one [`Buffer::indent_unit`] step.
    /// All inserts share one undo snapshot, so a five-line indent rewinds
    /// in a single `u`. Returns `true` when the buffer was modified.
    pub fn indent_selection(&mut self) -> bool {
        self.shift_block_indent(IndentDirection::Indent)
    }

    /// Inverse of [`Buffer::indent_selection`]: remove up to one indent
    /// unit of leading whitespace from each touched line. Lines with less
    /// leading whitespace than the inferred unit shrink to flush-left
    /// without touching content past the indent boundary. Single undo
    /// entry, `false` returned when no line had any indent to strip.
    pub fn dedent_selection(&mut self) -> bool {
        self.shift_block_indent(IndentDirection::Dedent)
    }

    fn shift_block_indent(&mut self, dir: IndentDirection) -> bool {
        if self.rope.len_lines() == 0 {
            return false;
        }
        let unit = self.indent_unit();
        if unit.is_empty() {
            return false;
        }
        let unit_chars = unit.chars().count();
        let (start_line, end_line) = self.selected_line_span();

        let cursor_line = self.cursor.line;
        let anchor_loc = self.selection_anchor.map(|idx| {
            let line = self.rope.char_to_line(idx);
            let line_start = self.rope.line_to_char(line);
            (line, idx - line_start)
        });

        let mut cursor_shift: isize = 0;
        let mut anchor_shift: isize = 0;
        let mut any_change = false;

        self.end_edit_group();
        self.begin_undo_group();

        for line_idx in start_line..=end_line {
            let line_start = self.rope.line_to_char(line_idx);
            let before = self.cursor;
            let delta = match dir {
                IndentDirection::Indent => {
                    self.record_insert(line_start, &unit);
                    self.rope.insert(line_start, &unit);
                    self.record_insert_delta(
                        line_start,
                        &unit,
                        before,
                        super::undo_log::GroupHint::Explicit,
                    );
                    any_change = true;
                    unit_chars as isize
                }
                IndentDirection::Dedent => {
                    let strip = count_leading_ws_to_remove(&self.rope, line_start, unit_chars);
                    if strip == 0 {
                        0
                    } else {
                        let end = line_start + strip;
                        let removed = self.rope.slice(line_start..end).to_string();
                        self.record_delete(line_start, end);
                        self.rope.remove(line_start..end);
                        self.record_delete_delta(
                            line_start,
                            &removed,
                            before,
                            super::undo_log::GroupHint::Explicit,
                        );
                        any_change = true;
                        -(strip as isize)
                    }
                }
            };
            if delta == 0 {
                continue;
            }
            if line_idx == cursor_line {
                cursor_shift = delta;
            }
            if let Some((aline, _)) = anchor_loc {
                if line_idx == aline {
                    anchor_shift = delta;
                }
            }
        }

        if !any_change {
            self.end_undo_group();
            return false;
        }

        self.cursor.col = shift_col(self.cursor.col, cursor_shift);
        self.cursor.desired_col = None;
        if let Some((aline, acol)) = anchor_loc {
            let new_acol = shift_col(acol, anchor_shift);
            self.selection_anchor = Some(self.rope.line_to_char(aline) + new_acol);
        }
        self.dirty = true;
        self.end_undo_group();
        true
    }

    /// Whole-line span covered by the active selection, or the current
    /// line as a degenerate one-line span when nothing is selected.
    /// Clamped to the buffer's last line so callers can iterate the
    /// inclusive range without further bounds checks.
    fn selected_line_span(&self) -> (usize, usize) {
        let max_line = self.rope.len_lines().saturating_sub(1);
        let Some((lo, hi)) = self.selection_range() else {
            let line = self.clamped_cursor_line();
            return (line, line);
        };
        let last_char = hi
            .saturating_sub(1)
            .min(self.rope.len_chars().saturating_sub(1));
        let start_line = self.rope.char_to_line(lo).min(max_line);
        let end_line = self.rope.char_to_line(last_char).min(max_line);
        (start_line, end_line)
    }

    pub(super) fn line_indent(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        self.rope
            .line(line)
            .chars()
            .take_while(|&ch| ch == ' ' || ch == '\t')
            .collect()
    }

    /// Inferred indent unit (`"\t"` or N spaces). The first tab-indented line
    /// short-circuits to `"\t"`; otherwise the gcd of observed leading-space
    /// widths defines the unit, falling back to 4 spaces when no indented
    /// content is found.
    pub(super) fn indent_unit(&self) -> String {
        let mut widths: Vec<usize> = Vec::new();
        let scan = self.rope.len_lines().min(INDENT_SCAN_LINES);
        for i in 0..scan {
            let line = self.rope.line(i);
            let mut spaces = 0usize;
            for ch in line.chars() {
                match ch {
                    '\t' => return "\t".to_string(),
                    ' ' => spaces += 1,
                    _ => break,
                }
            }
            let has_content = line
                .chars()
                .nth(spaces)
                .is_some_and(|c| c != '\n' && c != '\r');
            if spaces > 0 && has_content {
                widths.push(spaces);
            }
        }
        let Some(unit) = widths.iter().copied().reduce(gcd) else {
            return FALLBACK_INDENT.to_string();
        };
        " ".repeat(unit.clamp(MIN_INDENT_WIDTH, MAX_INDENT_WIDTH))
    }

    pub(super) fn last_non_ws_char_on_line(&self, line: usize) -> Option<char> {
        if line >= self.rope.len_lines() {
            return None;
        }
        self.rope
            .line(line)
            .chars()
            .take_while(|&ch| ch != '\n' && ch != '\r')
            .filter(|ch| !is_indent_ws(*ch))
            .last()
    }

    pub(super) fn last_non_ws_before_cursor(&self) -> Option<char> {
        let line = self.clamped_cursor_line();
        let line_start = self.rope.line_to_char(line);
        let cursor = self.char_index();
        (line_start..cursor)
            .rev()
            .map(|i| self.rope.char(i))
            .find(|ch| !is_indent_ws(*ch))
    }

    /// First non-blank char between the cursor and the **end of the current
    /// line** — intentionally line-scoped. The smart-Enter pair-expansion in
    /// `build_indented_newline` uses this to confirm the closer is sitting
    /// right next to the opener, so an already-multiline `fn foo(\n|\n)`
    /// does NOT trigger a fresh expansion (the user already broke the pair
    /// across lines). The single-line-pair invariant is pinned by
    /// `smart_enter_does_not_expand_across_existing_newlines` in
    /// `tests/buffer.rs`.
    pub(super) fn first_non_ws_after_cursor(&self) -> Option<char> {
        let line = self.clamped_cursor_line();
        let line_end = self.rope.line_to_char(line) + self.line_char_len(line);
        (self.char_index()..line_end)
            .map(|i| self.rope.char(i))
            .take_while(|&ch| ch != '\n' && ch != '\r')
            .find(|ch| !is_indent_ws(*ch))
    }

    /// Canonical language label (e.g. `"python"`, `"rust"`) inferred from
    /// the buffer's path, or `None` for scratch buffers or unknown
    /// extensions. Used by [`is_block_starter`] to gate language-specific
    /// indent rules.
    pub(super) fn language_label(&self) -> Option<&'static str> {
        self.path
            .as_deref()
            .and_then(crate::languages::detect_by_path)
            .map(|info| info.label)
    }

    /// True when the cursor's line — taken from line start up to the cursor
    /// column — is a control-flow terminator statement: `return`, `break`,
    /// `continue`, plus Python's `pass` / `raise`. The smart-newline path
    /// uses this to dedent the line that follows a terminator one indent
    /// unit relative to the terminator's own level, since the surrounding
    /// block has logically ended.
    pub(super) fn line_prefix_ends_with_terminator(&self, language: Option<&str>) -> bool {
        let line = self.clamped_cursor_line();
        let line_start = self.rope.line_to_char(line);
        let cursor = self.char_index();
        if cursor <= line_start {
            return false;
        }
        let prefix: String = (line_start..cursor).map(|i| self.rope.char(i)).collect();
        is_block_terminator(&prefix, language)
    }
}

pub(super) fn is_indent_opener(ch: char) -> bool {
    matches!(ch, '{' | '(' | '[')
}

/// Decide whether `ch` — the last non-whitespace char on the line being
/// left — should bump the next line one indent level deeper. Brackets
/// (`{ ( [`) always trigger. Language-specific block openers (Python's
/// `:`) only trigger when the buffer's language matches.
///
/// Adding a new language rule is one match arm — e.g. Lua's `then` /
/// `do` would parse the trailing word rather than a single char, so a
/// new branch keyed on `Some("lua")` slots in here.
pub(super) fn is_block_starter(ch: char, language: Option<&str>) -> bool {
    if is_indent_opener(ch) {
        return true;
    }
    match language {
        Some("python") => ch == ':',
        _ => false,
    }
}

/// Counterpart to [`is_block_starter`]: detect lines whose semantic shape
/// closes the surrounding block, so the line that follows should dedent
/// one step. Recognises `return`, `break`, `continue` universally and
/// Python's `pass` / `raise` when the buffer is `.py`. The check looks
/// for the keyword at the *start* of the trimmed prefix followed by a
/// word boundary, so `return foo` and `return foo;` both qualify while
/// `return_value = 1` does not.
pub(super) fn is_block_terminator(line_text: &str, language: Option<&str>) -> bool {
    let trimmed = line_text.trim();
    if trimmed.is_empty() {
        return false;
    }
    for kw in terminator_keywords(language) {
        if let Some(rest) = trimmed.strip_prefix(kw) {
            if rest.chars().next().is_none_or(|c| !is_word_continuation(c)) {
                return true;
            }
        }
    }
    false
}

fn terminator_keywords(language: Option<&str>) -> &'static [&'static str] {
    match language {
        Some("python") => &["return", "break", "continue", "pass", "raise"],
        _ => &["return", "break", "continue"],
    }
}

fn is_word_continuation(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

pub(super) fn matching_closer(opener: char) -> Option<char> {
    match opener {
        '{' => Some('}'),
        '(' => Some(')'),
        '[' => Some(']'),
        _ => None,
    }
}

/// Matched-pair lookup spanning bracket *and* quote pairs. Used by the
/// auto-pair-aware backspace path: deleting an opener that sits immediately
/// before its closer removes both as one edit. Quotes are intentionally
/// excluded from [`is_indent_opener`] / [`matching_closer`] because the
/// smart-newline + `o`-line expansion that those helpers gate is only
/// meaningful for code blocks, not string literals.
pub(super) fn pair_opener_closer(opener: char) -> Option<char> {
    match opener {
        '{' => Some('}'),
        '(' => Some(')'),
        '[' => Some(']'),
        '"' => Some('"'),
        '\'' => Some('\''),
        _ => None,
    }
}

fn is_indent_ws(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

#[derive(Copy, Clone)]
enum IndentDirection {
    Indent,
    Dedent,
}

/// How many leading-whitespace chars to strip from `line_start` to peel off
/// one indent step. A tab counts as one full step regardless of its visual
/// width, mirroring vim's `<<` on a tab-indented line; space-indented lines
/// give up to `unit_chars` consecutive spaces.
fn count_leading_ws_to_remove(rope: &ropey::Rope, line_start: usize, unit_chars: usize) -> usize {
    let mut idx = line_start;
    let len = rope.len_chars();
    if idx >= len {
        return 0;
    }
    let first = rope.char(idx);
    if first == '\t' {
        return 1;
    }
    let mut taken = 0;
    while taken < unit_chars && idx < len && rope.char(idx) == ' ' {
        idx += 1;
        taken += 1;
    }
    taken
}

fn shift_col(col: usize, delta: isize) -> usize {
    if delta >= 0 {
        col + delta as usize
    } else {
        col.saturating_sub((-delta) as usize)
    }
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}
