use super::Buffer;

/// Hard cap on the rope scan, in chars. Prevents pathological cost on a
/// minified-on-one-line bundle when the partner is hundreds of KB away.
/// 16k chars covers ~200 normal source lines in either direction — anything
/// larger almost never reads as a "matched pair" to a human eye anyway.
const SCAN_LIMIT: usize = 16_384;

/// A located bracket and its match. Indices are char positions in the rope;
/// the renderer converts them back to (line, col) via `Buffer::char_to_point`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BracketMatch {
    pub cursor_idx: usize,
    pub partner_idx: usize,
}

#[derive(Copy, Clone)]
enum Direction {
    Forward,
    Backward,
}

/// Resolve the bracket the cursor is currently *on* (if any) to its partner.
/// Returns `None` when the cursor isn't on a bracket char, when the pair is
/// unbalanced, or when the scan limit was exhausted before a match was found
/// — the renderer treats all three the same (no highlight, no panic).
pub fn find_matching_bracket(buf: &Buffer, line: usize, col: usize) -> Option<BracketMatch> {
    let rope = buf.rope_ref();
    if line >= rope.len_lines() {
        return None;
    }
    let line_start = rope.line_to_char(line);
    let line_len = buf.line_char_len(line);
    if col >= line_len {
        return None;
    }
    let cursor_idx = line_start + col;
    let here = rope.char(cursor_idx);
    let (opener, closer, dir) = classify_bracket(here)?;
    let partner_idx = match dir {
        Direction::Forward => scan(rope, cursor_idx, opener, closer, dir)?,
        Direction::Backward => scan(rope, cursor_idx, closer, opener, dir)?,
    };
    Some(BracketMatch {
        cursor_idx,
        partner_idx,
    })
}

/// Map a bracket char to (opener, closer, scan-direction). Angle brackets are
/// intentionally excluded — `<` and `>` are ambiguous (generics, comparisons,
/// HTML tags) and a naive pair scan flickers on every `a < b` expression.
/// Keep them out until syntax-info-aware matching lands.
fn classify_bracket(ch: char) -> Option<(char, char, Direction)> {
    match ch {
        '(' => Some(('(', ')', Direction::Forward)),
        '[' => Some(('[', ']', Direction::Forward)),
        '{' => Some(('{', '}', Direction::Forward)),
        ')' => Some(('(', ')', Direction::Backward)),
        ']' => Some(('[', ']', Direction::Backward)),
        '}' => Some(('{', '}', Direction::Backward)),
        _ => None,
    }
}

/// Depth-tracked walk through the rope. `same` is the bracket whose
/// appearance raises depth (the one identical to the cursor's), `other` is
/// the partner whose appearance lowers it. Walks forward from `start + 1`
/// or backward from `start - 1` and returns the index where depth balances.
fn scan(
    rope: &ropey::Rope,
    start: usize,
    same: char,
    other: char,
    dir: Direction,
) -> Option<usize> {
    let total = rope.len_chars();
    let mut depth: usize = 1;
    let (mut idx, step) = match dir {
        Direction::Forward => (start.checked_add(1)?, 1isize),
        Direction::Backward => (start.checked_sub(1)?, -1isize),
    };
    let mut budget = SCAN_LIMIT;
    while budget > 0 {
        if idx >= total {
            return None;
        }
        let ch = rope.char(idx);
        if ch == same {
            depth += 1;
        } else if ch == other {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
        budget -= 1;
        match step {
            1 => idx = idx.checked_add(1)?,
            _ => {
                if idx == 0 {
                    return None;
                }
                idx -= 1;
            }
        }
    }
    None
}

impl Buffer {
    pub(super) fn rope_ref(&self) -> &ropey::Rope {
        &self.rope
    }

    /// Convert a rope char index back to `(line, col)`. Returns `None` when
    /// `idx` is past end-of-rope; the renderer treats that the same as "no
    /// partner found".
    pub fn char_to_point(&self, idx: usize) -> Option<(usize, usize)> {
        if idx > self.rope.len_chars() {
            return None;
        }
        let line = self.rope.char_to_line(idx);
        let line_start = self.rope.line_to_char(line);
        Some((line, idx - line_start))
    }
}
