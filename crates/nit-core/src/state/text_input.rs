//! Single-line text input widgets shared between the editor's `/` search
//! prompt and the `:` command line.

/// Vim-style in-editor search state. Tracks the active term, whether `*`/`#`
/// started it (which forces whole-word matching), and the last direction so
/// `n` repeats it and `N` reverses. Shared across editor buffers, matching
/// vim's globally-scoped last-search pattern.
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

#[derive(Clone, Debug, Default)]
pub struct SearchPrompt {
    pub input: String,
    pub cursor: usize,
}

impl SearchPrompt {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, ch: char) {
        insert_char_at_cursor(&mut self.input, &mut self.cursor, ch);
    }

    pub fn backspace(&mut self) {
        backspace_at_cursor(&mut self.input, &mut self.cursor);
    }
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
