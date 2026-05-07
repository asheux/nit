/// Vim-style in-editor search state.
///
/// Tracks the active search term, whether it was started by `*` / `#` (which
/// implies whole-word matching), and the last direction (so `n` repeats in
/// the same direction and `N` reverses it). The state is shared across all
/// editor buffers, matching vim's behaviour where the last search pattern is
/// global to the editor.
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

/// `/` search prompt input state. Mirrors `CommandLine` but for buffer search.
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
        let idx = char_idx_to_byte(&self.input, self.cursor);
        self.input.insert(idx, ch);
        self.cursor = self.cursor.saturating_add(1);
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = char_idx_to_byte(&self.input, self.cursor);
        let start = char_idx_to_byte(&self.input, self.cursor.saturating_sub(1));
        if start < end {
            self.input.replace_range(start..end, "");
            self.cursor = self.cursor.saturating_sub(1);
        }
    }
}

#[derive(Clone, Debug)]
pub struct CommandLine {
    pub input: String,
    pub cursor: usize,
}

impl CommandLine {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
        }
    }

    pub fn insert(&mut self, ch: char) {
        let idx = char_idx_to_byte(&self.input, self.cursor);
        self.input.insert(idx, ch);
        self.cursor = self.cursor.saturating_add(1);
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = char_idx_to_byte(&self.input, self.cursor);
        let start = char_idx_to_byte(&self.input, self.cursor.saturating_sub(1));
        if start < end {
            self.input.replace_range(start..end, "");
            self.cursor = self.cursor.saturating_sub(1);
        }
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

impl Default for CommandLine {
    fn default() -> Self {
        Self::new()
    }
}

fn char_idx_to_byte(s: &str, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    for (count, (byte_idx, _)) in s.char_indices().enumerate() {
        if count == idx {
            return byte_idx;
        }
    }
    s.len()
}
