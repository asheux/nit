use std::collections::VecDeque;

/// Maximum number of jump records the ring retains. Matches vim's default
/// `'jumplist'` capacity.
pub const JUMPLIST_CAPACITY: usize = 100;

/// A single jumplist entry: where a jump-eligible motion left the cursor.
/// `buffer_id` is the `AppState::active_editor_buffer_id` index so the
/// ring can model cross-buffer file switches without depending on the
/// state crate's buffer storage layout.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JumpEntry {
    pub buffer_id: usize,
    pub line: usize,
    pub col: usize,
}

impl JumpEntry {
    pub fn new(buffer_id: usize, line: usize, col: usize) -> Self {
        Self {
            buffer_id,
            line,
            col,
        }
    }
}

/// Bounded jumplist with vim's truncate-on-push semantics: every `push`
/// discards anything after the current navigation cursor, so a new jump
/// after several `Ctrl-O`s replaces the abandoned forward branch.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct JumpList {
    #[serde(skip)]
    entries: VecDeque<JumpEntry>,
    #[serde(skip)]
    cursor: usize,
}

impl JumpList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn capacity() -> usize {
        JUMPLIST_CAPACITY
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Append `entry` to the ring. Drops the forward tail that may have
    /// existed past `cursor`, deduplicates a consecutive identical entry,
    /// and evicts the oldest record when the capacity is exceeded.
    pub fn push(&mut self, entry: JumpEntry) {
        if self.cursor < self.entries.len() {
            self.entries.truncate(self.cursor);
        }
        if self.entries.back() == Some(&entry) {
            self.cursor = self.entries.len();
            return;
        }
        self.entries.push_back(entry);
        while self.entries.len() > JUMPLIST_CAPACITY {
            self.entries.pop_front();
        }
        self.cursor = self.entries.len();
    }

    /// Return the previous entry (vim `Ctrl-O`), advancing the navigation
    /// cursor backwards. Returns `None` when there's nothing older.
    pub fn jump_back(&mut self) -> Option<JumpEntry> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        self.entries.get(self.cursor).copied()
    }

    /// Return the next entry (vim `Ctrl-I`), advancing the navigation
    /// cursor forwards. Returns `None` when there is no newer record to
    /// resume.
    pub fn jump_forward(&mut self) -> Option<JumpEntry> {
        if self.cursor + 1 > self.entries.len() {
            return None;
        }
        if self.cursor + 1 == self.entries.len() {
            // Already aligned with the most recent record.
            return None;
        }
        self.cursor += 1;
        self.entries.get(self.cursor).copied()
    }

    /// Inspect (without mutating) the entry currently focused by the
    /// navigation cursor. Used by tests and any UI that needs to label
    /// the active jumplist row.
    pub fn current(&self) -> Option<JumpEntry> {
        if self.cursor == 0 || self.cursor > self.entries.len() {
            return None;
        }
        self.entries.get(self.cursor - 1).copied()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = 0;
    }
}
