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
}

pub(super) fn is_indent_opener(ch: char) -> bool {
    matches!(ch, '{' | '(' | '[')
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

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}
