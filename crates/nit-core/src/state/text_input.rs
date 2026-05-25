//! Single-line text input widgets shared between the editor's `/` search
//! prompt and the `:` command line.

use crate::cursor::Cursor;

#[derive(Clone, Debug, Default)]
pub struct EditorSearch {
    pub term: Option<String>,
    pub whole_word: bool,
    pub forward: bool,
}

impl EditorSearch {
    pub fn is_active(&self) -> bool {
        self.term.as_deref().is_some_and(|t| !t.is_empty())
    }

    pub fn clear(&mut self) {
        self.term = None;
        self.whole_word = false;
    }
}

/// `/` prompt state. Holds the in-progress query, an insertion cursor, and
/// the cursor position from before the prompt opened so `Esc` can restore
/// it (vim's incremental-search semantics).
#[derive(Clone, Debug, Default)]
pub struct SearchPrompt {
    pub input: String,
    pub cursor: usize,
    /// Cursor + buffer the user was on when `/` opened. Used by `Esc` to
    /// jump back; persists through every keystroke until the prompt closes.
    pub pre_search_cursor: Option<(usize, Cursor)>,
}

impl SearchPrompt {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_origin(buffer_id: usize, cursor: Cursor) -> Self {
        Self {
            pre_search_cursor: Some((buffer_id, cursor)),
            ..Self::default()
        }
    }

    pub fn insert(&mut self, ch: char) {
        insert_char_at_cursor(&mut self.input, &mut self.cursor, ch);
    }

    pub fn backspace(&mut self) {
        backspace_at_cursor(&mut self.input, &mut self.cursor);
    }

    /// Append a bracketed-paste payload, dropping anything after the first
    /// newline. Vim treats `/` as single-line, so a pasted multi-line blob
    /// becomes its leading line.
    pub fn append_paste(&mut self, text: &str) {
        let head = text.split(['\n', '\r']).next().unwrap_or("");
        for ch in head.chars() {
            self.insert(ch);
        }
    }

    /// Vim smart-case: lowercase-only query → case-insensitive; any uppercase
    /// char → case-sensitive. Mirrors `:set smartcase`.
    pub fn case_insensitive(&self) -> bool {
        smart_case_insensitive(&self.input)
    }
}

/// Returns `true` when `term` should match case-insensitively under vim's
/// smart-case rule. Exposed at module scope so callers that don't have a
/// `SearchPrompt` handy (e.g. the `editor_search.term` highlight path on
/// the next render) can apply the same logic.
pub fn smart_case_insensitive(term: &str) -> bool {
    !term.chars().any(|c| c.is_uppercase())
}

#[derive(Clone, Debug, Default)]
pub struct CommandLine {
    pub input: String,
    pub cursor: usize,
}

impl CommandLine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, ch: char) {
        insert_char_at_cursor(&mut self.input, &mut self.cursor, ch);
    }

    pub fn backspace(&mut self) {
        backspace_at_cursor(&mut self.input, &mut self.cursor);
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        let len = self.input.chars().count();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    /// `:` is single-line; a paste with embedded newlines truncates at the
    /// first one (vim ex-line parity).
    pub fn append_paste(&mut self, text: &str) {
        let head = text.split(['\n', '\r']).next().unwrap_or("");
        for ch in head.chars() {
            self.insert(ch);
        }
    }
}

fn insert_char_at_cursor(input: &mut String, cursor: &mut usize, ch: char) {
    let byte_idx = char_idx_to_byte(input, *cursor);
    input.insert(byte_idx, ch);
    *cursor = cursor.saturating_add(1);
}

fn backspace_at_cursor(input: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let end = char_idx_to_byte(input, *cursor);
    let start = char_idx_to_byte(input, cursor.saturating_sub(1));
    if start < end {
        input.replace_range(start..end, "");
        *cursor = cursor.saturating_sub(1);
    }
}

fn char_idx_to_byte(s: &str, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    s.char_indices()
        .nth(idx)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(s.len())
}
