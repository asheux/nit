use crate::{cursor::Cursor, viewport::Viewport};
use ropey::Rope;
use std::path::PathBuf;
use unicode_segmentation::UnicodeSegmentation;

const UNDO_LIMIT: usize = 256;

/// Per-line diff status relative to the base (on-disk) content.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LineDiffStatus {
    Unchanged,
    Added,
    Modified,
    /// One or more lines were deleted just above this line.
    DeletedAbove,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BufferPoint {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
pub struct BufferEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: BufferPoint,
    pub old_end_point: BufferPoint,
    pub new_end_point: BufferPoint,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum EditKind {
    Insert,
}

#[derive(Copy, Clone, Debug)]
struct EditMeta {
    kind: EditKind,
    cursor_index: usize,
}

#[derive(Clone, Debug)]
struct Snapshot {
    rope: Rope,
    cursor: Cursor,
    dirty: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Buffer {
    name: String,
    path: Option<PathBuf>,
    #[serde(skip)]
    rope: Rope,
    #[serde(skip)]
    undo: Vec<Snapshot>,
    #[serde(skip)]
    redo: Vec<Snapshot>,
    #[serde(skip)]
    last_edit: Option<EditMeta>,
    #[serde(skip)]
    pending_edits: Vec<BufferEdit>,
    #[serde(skip)]
    full_reparse: bool,
    #[serde(skip)]
    version: u64,
    #[serde(skip)]
    selection_anchor: Option<usize>,
    pub cursor: Cursor,
    pub viewport: Viewport,
    dirty: bool,
    /// Line hashes of the base (on-disk) content for diff computation.
    #[serde(skip)]
    base_line_hashes: Vec<u64>,
    /// Cached diff status per line. Recomputed when version changes.
    #[serde(skip)]
    diff_status: Vec<LineDiffStatus>,
    /// Buffer version at which diff_status was last computed.
    #[serde(skip)]
    diff_version: u64,
}

impl Buffer {
    pub fn new(name: impl Into<String>, content: Rope, path: Option<PathBuf>) -> Self {
        let base_hashes = rope_line_hashes(&content);
        Self {
            name: name.into(),
            path,
            rope: content,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            pending_edits: Vec::new(),
            full_reparse: false,
            version: 0,
            selection_anchor: None,
            cursor: Cursor::default(),
            viewport: Viewport::default(),
            dirty: false,
            base_line_hashes: base_hashes,
            diff_status: Vec::new(),
            diff_version: u64::MAX, // force initial computation
        }
    }

    pub fn empty(name: impl Into<String>, path: Option<PathBuf>) -> Self {
        Self::new(name, Rope::new(), path)
    }

    pub fn from_str(name: impl Into<String>, content: &str, path: Option<PathBuf>) -> Self {
        Self::new(name, Rope::from_str(content), path)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
    }

    pub fn set_viewport_size(&mut self, height: usize, width: usize) {
        self.viewport.height = height;
        self.viewport.width = width;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn take_pending_edits(&mut self) -> Vec<BufferEdit> {
        std::mem::take(&mut self.pending_edits)
    }

    pub fn take_full_reparse(&mut self) -> bool {
        let flag = self.full_reparse;
        self.full_reparse = false;
        flag
    }

    pub fn bytes_len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn lines_len(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn content_as_string(&self) -> String {
        self.rope.to_string()
    }

    pub fn first_line(&self) -> Option<String> {
        if self.rope.len_lines() == 0 {
            return None;
        }
        let mut line = self.rope.line(0).to_string();
        if line.ends_with('\n') {
            line.pop();
        }
        Some(line)
    }

    pub fn grapheme_width_at_line(&self, line: usize) -> usize {
        let line_text = self.line_as_str(line);
        line_text.graphemes(true).count()
    }

    pub fn line_as_string(&self, line: usize) -> String {
        self.line_as_str(line)
    }

    pub fn line_hash(&self, line: usize) -> u64 {
        const OFFSET: u64 = 14695981039346656037;
        const PRIME: u64 = 1099511628211;
        if line >= self.rope.len_lines() {
            return OFFSET;
        }
        let mut hash = OFFSET;
        // Ropey stores text in UTF-8 chunks. Hash incrementally to avoid allocating a String.
        // Match nit_syntax::hash_line_bytes semantics: ignore trailing '\n' and skip '\r'.
        for chunk in self.rope.line(line).chunks() {
            for &b in chunk.as_bytes() {
                if b == b'\n' || b == b'\r' {
                    continue;
                }
                hash ^= b as u64;
                hash = hash.wrapping_mul(PRIME);
            }
        }
        hash
    }

    /// Get the diff status for a specific line. Lazily recomputes when buffer changes.
    pub fn line_diff_status(&mut self, line: usize) -> LineDiffStatus {
        self.ensure_diff_computed();
        self.diff_status
            .get(line)
            .copied()
            .unwrap_or(LineDiffStatus::Unchanged)
    }

    /// Check if any line has a diff status (i.e., buffer differs from base).
    pub fn has_diff(&mut self) -> bool {
        self.ensure_diff_computed();
        self.diff_status
            .iter()
            .any(|s| *s != LineDiffStatus::Unchanged)
    }

    /// Set the base content for diff computation from a git HEAD blob.
    pub fn set_git_base(&mut self, base_content: &str) {
        let base_rope = Rope::from_str(base_content);
        self.base_line_hashes = rope_line_hashes(&base_rope);
        self.diff_version = u64::MAX; // force recomputation
    }

    /// Get the cached diff status slice. Call `compute_diff_if_needed` first.
    pub fn diff_statuses(&self) -> &[LineDiffStatus] {
        &self.diff_status
    }

    /// Ensure diff is computed for the current version. Call before rendering.
    pub fn compute_diff_if_needed(&mut self) {
        self.ensure_diff_computed();
    }

    fn ensure_diff_computed(&mut self) {
        if self.diff_version == self.version {
            return;
        }
        let current_hashes: Vec<u64> = (0..self.rope.len_lines())
            .map(|i| self.line_hash(i))
            .collect();
        self.diff_status = compute_line_diff(&self.base_line_hashes, &current_hashes);
        self.diff_version = self.version;
    }

    pub fn line_char_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    pub fn line_char_end(&self, line: usize) -> usize {
        if line + 1 < self.rope.len_lines() {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        }
    }

    pub fn cursor_index(&self) -> usize {
        self.char_index()
    }

    fn line_as_str(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        self.rope.line(line).to_string()
    }

    fn line_char_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            0
        } else {
            let slice = self.rope.line(line);
            let mut len = slice.len_chars();
            if len > 0 && slice.chars().last() == Some('\n') {
                len = len.saturating_sub(1);
            }
            len
        }
    }

    fn char_index(&self) -> usize {
        let line_start = self.rope.line_to_char(self.cursor.line);
        let col = self.cursor.col.min(self.line_char_len(self.cursor.line));
        line_start + col
    }

    fn clamp_col(&mut self) {
        let len = self.line_char_len(self.cursor.line);
        if self.cursor.col > len {
            self.cursor.col = len;
        }
    }

    pub fn move_left(&mut self) {
        self.end_edit_group();
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.line_char_len(self.cursor.line);
        }
    }

    pub fn move_right(&mut self) {
        self.end_edit_group();
        let len = self.line_char_len(self.cursor.line);
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.rope.len_lines() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        self.end_edit_group();
        if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.clamp_col();
        }
    }

    pub fn move_down(&mut self) {
        self.end_edit_group();
        if self.cursor.line + 1 < self.rope.len_lines() {
            self.cursor.line += 1;
            self.clamp_col();
        }
    }

    pub fn page_up(&mut self, count: usize) {
        self.end_edit_group();
        let jump = count.min(self.cursor.line);
        self.cursor.line -= jump;
        self.clamp_col();
    }

    pub fn page_down(&mut self, count: usize) {
        self.end_edit_group();
        let max_line = self.rope.len_lines().saturating_sub(1);
        self.cursor.line = (self.cursor.line + count).min(max_line);
        self.clamp_col();
    }

    pub fn move_home(&mut self) {
        self.end_edit_group();
        self.cursor.col = 0;
    }

    pub fn move_end(&mut self) {
        self.end_edit_group();
        self.cursor.col = self.line_char_len(self.cursor.line);
    }

    pub fn append(&mut self) {
        self.end_edit_group();
        let len = self.line_char_len(self.cursor.line);
        if self.cursor.col < len {
            self.cursor.col += 1;
        }
    }

    pub fn set_selection_anchor(&mut self) {
        self.selection_anchor = Some(self.char_index());
    }

    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    pub fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor?;
        let cursor = self.char_index();
        let len = self.rope.len_chars();
        let (start, end) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        let end = if end < len { end + 1 } else { len };
        if start >= len || end <= start {
            None
        } else {
            Some((start, end))
        }
    }

    pub fn yank_selection(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        Some(self.rope.slice(start..end).to_string())
    }

    pub fn yank_line(&self) -> String {
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let start = self.rope.line_to_char(line);
        let end = if line + 1 < self.rope.len_lines() {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        };
        let mut text = self.rope.slice(start..end).to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }

    pub fn delete_selection(&mut self) -> bool {
        self.end_edit_group();
        let (start, end) = match self.selection_range() {
            Some(range) => range,
            None => return false,
        };
        if start >= end {
            return false;
        }
        self.record_delete(start, end);
        self.push_undo();
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.dirty = true;
        self.clear_selection();
        true
    }

    fn replace_selection_with_str(&mut self, s: &str) {
        let (start, end) = match self.selection_range() {
            Some(range) => range,
            None => return,
        };
        if start >= end {
            self.clear_selection();
            return;
        }

        self.end_edit_group();
        self.push_undo();

        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clear_selection();

        if s.is_empty() {
            self.dirty = true;
            return;
        }

        let idx = self.char_index();
        self.record_insert(idx, s);
        self.rope.insert(idx, s);
        let mut line = self.cursor.line;
        let mut col = self.cursor.col;
        for ch in s.chars() {
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        self.cursor.line = line;
        self.cursor.col = col;
        self.dirty = true;
        self.finish_insert_group();
    }

    fn replace_selection_with_newline_preserve_indent(&mut self) {
        let (start, end) = match self.selection_range() {
            Some(range) => range,
            None => return,
        };
        if start >= end {
            self.clear_selection();
            return;
        }

        self.end_edit_group();
        self.push_undo();

        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clear_selection();

        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);
        let idx = self.char_index();

        // Smart indent after selection replacement
        let char_before = self.last_non_ws_before_cursor();
        let should_increase = char_before.is_some_and(is_indent_opener);
        let char_after = self.first_non_ws_after_cursor();
        let bracket_pair = should_increase
            && char_before
                .and_then(matching_closer)
                .zip(char_after)
                .is_some_and(|(expected, actual)| expected == actual);

        let extra_indent = if should_increase {
            self.indent_unit()
        } else {
            String::new()
        };

        let mut text = String::from("\n");
        text.push_str(&indent);
        text.push_str(&extra_indent);
        if bracket_pair {
            text.push('\n');
            text.push_str(&indent);
        }

        self.record_insert(idx, &text);
        self.rope.insert(idx, &text);
        self.cursor.line += 1;
        self.cursor.col = indent.chars().count() + extra_indent.chars().count();
        self.dirty = true;
        self.finish_insert_group();
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.selection_range().is_some() {
            self.replace_selection_with_str(s);
            return;
        }
        let idx = self.char_index();
        self.record_insert(idx, s);
        self.begin_insert_group(idx);
        self.rope.insert(idx, s);
        let mut line = self.cursor.line;
        let mut col = self.cursor.col;
        for ch in s.chars() {
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        self.cursor.line = line;
        self.cursor.col = col;
        self.dirty = true;
        self.finish_insert_group();
    }

    pub fn open_line_below(&mut self) {
        self.end_edit_group();
        self.push_undo();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);

        // Smart indent: if line ends with an opener, increase indent
        let last_char = self.last_non_ws_char_on_line(line);
        let extra_indent = if last_char.is_some_and(|c| is_indent_opener(c) || c == ':') {
            self.indent_unit()
        } else {
            String::new()
        };

        let insert_at = self.rope.line_to_char(line) + self.line_char_len(line);
        let mut text = String::from("\n");
        text.push_str(&indent);
        text.push_str(&extra_indent);
        self.record_insert(insert_at, &text);
        self.rope.insert(insert_at, &text);
        self.cursor.line = line + 1;
        self.cursor.col = indent.chars().count() + extra_indent.chars().count();
        self.dirty = true;
    }

    pub fn open_line_above(&mut self) {
        self.end_edit_group();
        self.push_undo();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = if line > 0 {
            self.line_indent(line.saturating_sub(1))
        } else {
            String::new()
        };
        let idx = self.rope.line_to_char(line);
        let mut text = indent.clone();
        text.push('\n');
        self.record_insert(idx, &text);
        self.rope.insert(idx, &text);
        self.cursor.line = line;
        self.cursor.col = indent.chars().count();
        self.dirty = true;
    }

    pub fn paste_line_above(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.end_edit_group();
        self.push_undo();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let idx = self.rope.line_to_char(line);
        self.record_insert(idx, text);
        self.rope.insert(idx, text);
        self.cursor.line = line;
        self.cursor.col = 0;
        self.dirty = true;
        self.clamp_col();
    }

    pub fn paste_line_below(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.end_edit_group();
        self.push_undo();
        let total = self.rope.len_lines();
        let line = self.cursor.line.min(total.saturating_sub(1));
        let idx = if line + 1 < total {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        };
        let mut insert_text = String::new();
        if idx > 0 && self.rope.char(idx.saturating_sub(1)) != '\n' {
            insert_text.push('\n');
        }
        insert_text.push_str(text);
        self.record_insert(idx, &insert_text);
        self.rope.insert(idx, &insert_text);
        self.cursor.line = (line + 1).min(self.rope.len_lines().saturating_sub(1));
        self.cursor.col = 0;
        self.dirty = true;
        self.clamp_col();
    }

    fn line_indent(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        let mut indent = String::new();
        for ch in self.rope.line(line).chars() {
            if ch == '\n' {
                break;
            }
            if ch == ' ' || ch == '\t' {
                indent.push(ch);
            } else {
                break;
            }
        }
        indent
    }

    /// Detect the indent unit used in this buffer (e.g. "\t", "  ", "    ").
    fn indent_unit(&self) -> String {
        let max = self.rope.len_lines().min(200);
        let mut use_tabs = false;
        let mut widths = Vec::new();
        for i in 0..max {
            let line = self.rope.line(i);
            let mut spaces = 0usize;
            for ch in line.chars() {
                if ch == '\t' {
                    use_tabs = true;
                    break;
                } else if ch == ' ' {
                    spaces += 1;
                } else {
                    break;
                }
            }
            if use_tabs {
                break;
            }
            let has_content = line
                .chars()
                .nth(spaces)
                .is_some_and(|c| c != '\n' && c != '\r');
            if spaces > 0 && has_content {
                widths.push(spaces);
            }
        }
        if use_tabs {
            return "\t".to_string();
        }
        if widths.is_empty() {
            return "    ".to_string();
        }
        let mut g = widths[0];
        for &w in &widths[1..] {
            g = gcd(g, w);
        }
        " ".repeat(g.clamp(1, 8))
    }

    /// Last non-whitespace character on a line, ignoring trailing spaces/tabs/newlines.
    fn last_non_ws_char_on_line(&self, line: usize) -> Option<char> {
        if line >= self.rope.len_lines() {
            return None;
        }
        let mut result = None;
        for ch in self.rope.line(line).chars() {
            if ch == '\n' || ch == '\r' {
                break;
            }
            if ch != ' ' && ch != '\t' {
                result = Some(ch);
            }
        }
        result
    }

    /// Last non-whitespace character before the cursor on the current line.
    fn last_non_ws_before_cursor(&self) -> Option<char> {
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let idx = self.char_index();
        let mut i = idx;
        while i > line_start {
            let ch = self.rope.char(i - 1);
            if ch != ' ' && ch != '\t' {
                return Some(ch);
            }
            i -= 1;
        }
        None
    }

    /// First non-whitespace character after the cursor on the current line.
    fn first_non_ws_after_cursor(&self) -> Option<char> {
        let idx = self.char_index();
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let line_end_char = self.rope.line_to_char(line) + self.line_char_len(line);
        let mut i = idx;
        while i < line_end_char {
            let ch = self.rope.char(i);
            if ch == '\n' || ch == '\r' {
                return None;
            }
            if ch != ' ' && ch != '\t' {
                return Some(ch);
            }
            i += 1;
        }
        None
    }

    pub fn exit_insert_mode(&mut self) {
        self.end_edit_group();
        if self.is_line_blank(self.cursor.line) {
            self.cursor.col = 0;
        } else if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
    }

    pub fn go_to_top(&mut self) {
        self.end_edit_group();
        self.cursor.line = 0;
        self.clamp_col();
    }

    pub fn go_to_bottom(&mut self) {
        self.end_edit_group();
        let last = self.rope.len_lines().saturating_sub(1);
        self.cursor.line = last;
        self.clamp_col();
    }

    pub fn move_word_end(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index();
        if idx >= len {
            return;
        }
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if is_word(self.rope.char(idx)) && idx + 1 < len && !is_word(self.rope.char(idx + 1)) {
            idx += 1;
        }
        while idx < len && !is_word(self.rope.char(idx)) {
            idx += 1;
        }
        if idx >= len {
            return;
        }
        while idx + 1 < len && is_word(self.rope.char(idx + 1)) {
            idx += 1;
        }
        self.set_cursor_from_char_index(idx);
    }

    pub fn move_word_back(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index();
        if idx == 0 {
            return;
        }
        if idx >= len {
            idx = len.saturating_sub(1);
        }
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if is_word(self.rope.char(idx)) {
            if idx > 0 && !is_word(self.rope.char(idx - 1)) {
                idx = idx.saturating_sub(1);
            }
        } else {
            idx = idx.saturating_sub(1);
        }
        while idx > 0 && !is_word(self.rope.char(idx)) {
            idx = idx.saturating_sub(1);
        }
        if !is_word(self.rope.char(idx)) {
            return;
        }
        while idx > 0 && is_word(self.rope.char(idx - 1)) {
            idx = idx.saturating_sub(1);
        }
        self.set_cursor_from_char_index(idx);
    }

    pub fn insert_char(&mut self, c: char) {
        if self.selection_range().is_some() {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            self.replace_selection_with_str(s);
            return;
        }
        let idx = self.char_index();
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        self.record_insert(idx, s);
        self.begin_insert_group(idx);
        self.rope.insert_char(idx, c);
        self.cursor.col += 1;
        self.dirty = true;
        self.finish_insert_group();
    }

    pub fn insert_tab(&mut self) {
        self.insert_char('\t');
    }

    pub fn insert_newline(&mut self) {
        if self.selection_range().is_some() {
            self.replace_selection_with_newline_preserve_indent();
            return;
        }
        let line = self
            .cursor
            .line
            .min(self.rope.len_lines().saturating_sub(1));
        let indent = self.line_indent(line);
        let idx = self.char_index();

        // Smart indent: check if we should increase indent
        let char_before = self.last_non_ws_before_cursor();
        let should_increase = char_before.is_some_and(is_indent_opener);

        // Bracket pair expansion: cursor between matching brackets like {|}
        let char_after = self.first_non_ws_after_cursor();
        let bracket_pair = should_increase
            && char_before
                .and_then(matching_closer)
                .zip(char_after)
                .is_some_and(|(expected, actual)| expected == actual);

        let extra_indent = if should_increase {
            self.indent_unit()
        } else {
            String::new()
        };

        let mut text = String::from("\n");
        text.push_str(&indent);
        text.push_str(&extra_indent);
        if bracket_pair {
            // Add closing bracket line: \n + base_indent
            text.push('\n');
            text.push_str(&indent);
        }

        self.record_insert(idx, &text);
        self.begin_insert_group(idx);
        self.rope.insert(idx, &text);
        self.cursor.line += 1;
        self.cursor.col = indent.chars().count() + extra_indent.chars().count();
        self.dirty = true;
        self.finish_insert_group();
    }

    pub fn backspace(&mut self) {
        self.end_edit_group();
        if self.cursor.col > 0 {
            let idx = self.char_index();
            if idx > 0 {
                self.record_delete(idx - 1, idx);
                self.push_undo();
                self.rope.remove(idx - 1..idx);
                self.cursor.col -= 1;
                self.dirty = true;
            }
        } else if self.cursor.line > 0 {
            let prev_len = self.line_char_len(self.cursor.line - 1);
            let idx = self.char_index();
            if idx > 0 {
                self.record_delete(idx - 1, idx);
                self.push_undo();
                self.rope.remove(idx - 1..idx);
                self.cursor.line -= 1;
                self.cursor.col = prev_len;
                self.dirty = true;
            }
        }
    }

    pub fn delete_word_back(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let end = self.char_index().min(len);
        if end == 0 || len == 0 {
            return;
        }

        let is_word = |c: char| c.is_alphanumeric() || c == '_';

        let mut idx = end;
        while idx > 0 && self.rope.char(idx - 1).is_whitespace() {
            idx = idx.saturating_sub(1);
        }
        if idx == 0 {
            return;
        }
        if is_word(self.rope.char(idx - 1)) {
            while idx > 0 && is_word(self.rope.char(idx - 1)) {
                idx = idx.saturating_sub(1);
            }
        } else {
            while idx > 0 {
                let ch = self.rope.char(idx - 1);
                if ch.is_whitespace() || is_word(ch) {
                    break;
                }
                idx = idx.saturating_sub(1);
            }
        }

        let start = idx;
        if start >= end {
            return;
        }
        self.record_delete(start, end);
        self.push_undo();
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.dirty = true;
        self.clamp_col();
    }

    pub fn delete_forward(&mut self) {
        self.end_edit_group();
        let idx = self.char_index();
        let line_len = self.line_char_len(self.cursor.line);
        if self.cursor.col < line_len {
            self.record_delete(idx, idx + 1);
            self.push_undo();
            self.rope.remove(idx..idx + 1);
            self.dirty = true;
        } else if self.cursor.line + 1 < self.rope.len_lines() {
            // delete the newline
            self.record_delete(idx, idx + 1);
            self.push_undo();
            self.rope.remove(idx..idx + 1);
            self.dirty = true;
        }
    }

    pub fn delete_word_forward(&mut self) {
        self.end_edit_group();
        let len = self.rope.len_chars();
        let start = self.char_index().min(len);
        if start >= len || len == 0 {
            return;
        }

        let is_word = |c: char| c.is_alphanumeric() || c == '_';

        let mut idx = start;
        if self.rope.char(idx).is_whitespace() {
            while idx < len && self.rope.char(idx).is_whitespace() {
                idx += 1;
            }
        } else if is_word(self.rope.char(idx)) {
            while idx < len && is_word(self.rope.char(idx)) {
                idx += 1;
            }
        } else {
            while idx < len {
                let ch = self.rope.char(idx);
                if ch.is_whitespace() || is_word(ch) {
                    break;
                }
                idx += 1;
            }
        }

        let end = idx;
        if end <= start {
            return;
        }
        self.record_delete(start, end);
        self.push_undo();
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.dirty = true;
        self.clamp_col();
    }

    pub fn delete_line(&mut self) {
        self.end_edit_group();
        let total = self.rope.len_lines();
        if total == 0 {
            return;
        }
        self.push_undo();
        let line = self.cursor.line.min(total.saturating_sub(1));
        let start = self.rope.line_to_char(line);
        let end = if line + 1 < total {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        };
        if end > start {
            self.record_delete(start, end);
            self.rope.remove(start..end);
        }
        let new_total = self.rope.len_lines();
        if new_total == 0 {
            self.cursor.line = 0;
            self.cursor.col = 0;
        } else if self.cursor.line >= new_total {
            self.cursor.line = new_total.saturating_sub(1);
            self.cursor.col = 0;
        } else {
            self.cursor.col = 0;
        }
        self.dirty = true;
        self.clamp_col();
    }

    pub fn ensure_visible(&mut self) {
        self.viewport
            .ensure_visible(self.cursor.line, self.cursor.col);
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
        // After save, current content becomes the new base for diff
        self.base_line_hashes = rope_line_hashes(&self.rope);
        self.diff_version = u64::MAX; // force recomputation
    }

    /// Reload the buffer content from disk if the file has changed.
    /// Returns `true` if the buffer was updated.
    pub fn reload_from_disk(&mut self) -> bool {
        let Some(path) = self.path.as_ref() else {
            return false;
        };
        let Ok(content) = std::fs::read_to_string(path) else {
            return false;
        };
        let new_rope = Rope::from_str(&content);
        if self.rope == new_rope {
            return false;
        }
        self.rope = new_rope;
        self.version = self.version.wrapping_add(1);
        self.full_reparse = true;
        self.pending_edits.clear();
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.base_line_hashes = rope_line_hashes(&self.rope);
        self.diff_version = u64::MAX;
        true
    }

    pub fn undo(&mut self) -> bool {
        if let Some(snapshot) = self.undo.pop() {
            self.end_edit_group();
            self.push_redo();
            self.rope = snapshot.rope;
            self.cursor = snapshot.cursor;
            self.dirty = snapshot.dirty;
            self.clear_selection();
            self.record_full_reparse();
            return true;
        }
        false
    }

    fn push_undo(&mut self) {
        if self.undo.len() >= UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
        });
        self.redo.clear();
    }

    pub fn redo(&mut self) -> bool {
        if let Some(snapshot) = self.redo.pop() {
            self.end_edit_group();
            self.push_undo_without_clearing_redo();
            self.rope = snapshot.rope;
            self.cursor = snapshot.cursor;
            self.dirty = snapshot.dirty;
            self.clear_selection();
            self.record_full_reparse();
            return true;
        }
        false
    }

    fn push_redo(&mut self) {
        if self.redo.len() >= UNDO_LIMIT {
            self.redo.remove(0);
        }
        self.redo.push(Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
        });
    }

    fn push_undo_without_clearing_redo(&mut self) {
        if self.undo.len() >= UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
        });
    }

    fn begin_insert_group(&mut self, idx: usize) {
        let start_new = match self.last_edit {
            Some(meta) => meta.kind != EditKind::Insert || meta.cursor_index != idx,
            None => true,
        };
        if start_new {
            self.push_undo();
        }
    }

    fn finish_insert_group(&mut self) {
        let cursor_index = self.char_index();
        self.last_edit = Some(EditMeta {
            kind: EditKind::Insert,
            cursor_index,
        });
    }

    fn end_edit_group(&mut self) {
        self.last_edit = None;
    }

    pub fn break_undo_group(&mut self) {
        self.end_edit_group();
    }

    fn record_insert(&mut self, start_char: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let start_byte = self.rope.char_to_byte(start_char);
        let start_point = self.point_from_char_index(start_char);
        let (new_end_byte, new_end_point) = advance_point(start_byte, start_point, text);
        let edit = BufferEdit {
            start_byte,
            old_end_byte: start_byte,
            new_end_byte,
            start_point,
            old_end_point: start_point,
            new_end_point,
        };
        self.push_edit(edit);
    }

    fn record_delete(&mut self, start_char: usize, end_char: usize) {
        if start_char >= end_char {
            return;
        }
        let start_byte = self.rope.char_to_byte(start_char);
        let old_end_byte = self.rope.char_to_byte(end_char);
        let start_point = self.point_from_char_index(start_char);
        let old_end_point = self.point_from_char_index(end_char);
        let edit = BufferEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte,
            start_point,
            old_end_point,
            new_end_point: start_point,
        };
        self.push_edit(edit);
    }

    fn push_edit(&mut self, edit: BufferEdit) {
        self.pending_edits.push(edit);
        self.version = self.version.wrapping_add(1);
    }

    fn record_full_reparse(&mut self) {
        self.pending_edits.clear();
        self.full_reparse = true;
        self.version = self.version.wrapping_add(1);
    }

    fn point_from_char_index(&self, idx: usize) -> BufferPoint {
        let line = self.rope.char_to_line(idx);
        let line_start_char = self.rope.line_to_char(line);
        let line_start_byte = self.rope.char_to_byte(line_start_char);
        let byte = self.rope.char_to_byte(idx);
        BufferPoint {
            row: line,
            column: byte.saturating_sub(line_start_byte),
        }
    }

    fn set_cursor_from_char_index(&mut self, idx: usize) {
        let line = self.rope.char_to_line(idx);
        let line_start = self.rope.line_to_char(line);
        self.cursor.line = line;
        self.cursor.col = idx.saturating_sub(line_start);
        self.clamp_col();
    }

    fn is_line_blank(&self, line: usize) -> bool {
        if line >= self.rope.len_lines() {
            return true;
        }
        let text = self.rope.line(line).to_string();
        text.trim_end_matches('\n').trim().is_empty()
    }
}

fn advance_point(start_byte: usize, start_point: BufferPoint, text: &str) -> (usize, BufferPoint) {
    let mut row = start_point.row;
    let mut column = start_point.column;
    let mut byte = start_byte;
    for b in text.bytes() {
        byte += 1;
        if b == b'\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (byte, BufferPoint { row, column })
}

fn rope_line_hashes(rope: &Rope) -> Vec<u64> {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let n = rope.len_lines();
    let mut hashes = Vec::with_capacity(n);
    for i in 0..n {
        let mut hash = OFFSET;
        for chunk in rope.line(i).chunks() {
            for &b in chunk.as_bytes() {
                if b == b'\n' || b == b'\r' {
                    continue;
                }
                hash ^= b as u64;
                hash = hash.wrapping_mul(PRIME);
            }
        }
        hashes.push(hash);
    }
    hashes
}

fn compute_line_diff(base: &[u64], current: &[u64]) -> Vec<LineDiffStatus> {
    if base.is_empty() {
        return vec![LineDiffStatus::Added; current.len()];
    }
    if current.is_empty() {
        return Vec::new();
    }

    // Patience diff: anchor on unique lines, recurse between anchors
    let mut mapping: Vec<Option<usize>> = vec![None; current.len()];
    patience_match(base, current, 0, base.len(), 0, current.len(), &mut mapping);
    build_diff_status(base, current, &mapping)
}

/// Patience diff: match equal prefix/suffix, anchor on unique common lines, recurse.
fn patience_match(
    base: &[u64],
    current: &[u64],
    b_lo: usize,
    b_hi: usize,
    c_lo: usize,
    c_hi: usize,
    result: &mut [Option<usize>],
) {
    if b_lo >= b_hi || c_lo >= c_hi {
        return;
    }

    // Match equal prefix
    let mut prefix = 0;
    while b_lo + prefix < b_hi
        && c_lo + prefix < c_hi
        && base[b_lo + prefix] == current[c_lo + prefix]
    {
        result[c_lo + prefix] = Some(b_lo + prefix);
        prefix += 1;
    }

    // Match equal suffix
    let mut suffix = 0;
    while b_lo + prefix < b_hi.saturating_sub(suffix)
        && c_lo + prefix < c_hi.saturating_sub(suffix)
        && base[b_hi - 1 - suffix] == current[c_hi - 1 - suffix]
    {
        result[c_hi - 1 - suffix] = Some(b_hi - 1 - suffix);
        suffix += 1;
    }

    let b_lo = b_lo + prefix;
    let b_hi = b_hi.saturating_sub(suffix);
    let c_lo = c_lo + prefix;
    let c_hi = c_hi.saturating_sub(suffix);

    if b_lo >= b_hi || c_lo >= c_hi {
        return;
    }

    // Count occurrences of each hash in the remaining ranges
    let mut base_positions: std::collections::HashMap<u64, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, &h) in base.iter().enumerate().take(b_hi).skip(b_lo) {
        base_positions.entry(h).or_default().push(i);
    }
    let mut curr_count: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for &h in &current[c_lo..c_hi] {
        *curr_count.entry(h).or_default() += 1;
    }

    // Find lines unique in both ranges → patience anchors
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (j, &h) in current.iter().enumerate().take(c_hi).skip(c_lo) {
        if let Some(positions) = base_positions.get(&h) {
            if positions.len() == 1 && curr_count.get(&h) == Some(&1) {
                pairs.push((positions[0], j));
            }
        }
    }

    // LIS on base indices to get ordered anchors
    let anchors = lis_by_first(&pairs);

    if anchors.is_empty() {
        // No unique anchors: fall back to greedy for this segment
        greedy_match(base, current, b_lo, b_hi, c_lo, c_hi, result);
        return;
    }

    // Set anchor matches and recurse between them
    let mut b_prev = b_lo;
    let mut c_prev = c_lo;
    for &(bi, ci) in &anchors {
        patience_match(base, current, b_prev, bi, c_prev, ci, result);
        result[ci] = Some(bi);
        b_prev = bi + 1;
        c_prev = ci + 1;
    }
    patience_match(base, current, b_prev, b_hi, c_prev, c_hi, result);
}

/// Greedy forward matching for small ranges without unique anchors.
fn greedy_match(
    base: &[u64],
    current: &[u64],
    b_lo: usize,
    b_hi: usize,
    c_lo: usize,
    c_hi: usize,
    result: &mut [Option<usize>],
) {
    let mut i = b_lo;
    let mut j = c_lo;
    let window = (b_hi - b_lo).max(c_hi - c_lo).min(50);
    while i < b_hi && j < c_hi {
        if base[i] == current[j] {
            result[j] = Some(i);
            i += 1;
            j += 1;
            continue;
        }
        let mut next_i = None;
        for di in 1..=window {
            let idx = i + di;
            if idx >= b_hi {
                break;
            }
            if base[idx] == current[j] {
                next_i = Some(idx);
                break;
            }
        }
        let mut next_j = None;
        for dj in 1..=window {
            let idx = j + dj;
            if idx >= c_hi {
                break;
            }
            if current[idx] == base[i] {
                next_j = Some(idx);
                break;
            }
        }
        match (next_i, next_j) {
            (Some(ni), Some(nj)) => {
                if ni - i <= nj - j {
                    i = ni;
                } else {
                    j = nj;
                }
            }
            (Some(ni), None) => i = ni,
            (None, Some(nj)) => j = nj,
            (None, None) => j += 1,
        }
    }
}

/// Longest increasing subsequence on (base_idx, current_idx) pairs, ordered by base_idx.
fn lis_by_first(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if pairs.is_empty() {
        return Vec::new();
    }
    // Standard patience sorting LIS on the first element
    let mut tails: Vec<usize> = Vec::new(); // indices into pairs
    let mut predecessors: Vec<Option<usize>> = vec![None; pairs.len()];

    for (idx, &(bi, _)) in pairs.iter().enumerate() {
        let pos = tails.partition_point(|&t| pairs[t].0 < bi);
        if pos > 0 {
            predecessors[idx] = Some(tails[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(idx);
        } else {
            tails[pos] = idx;
        }
    }

    // Reconstruct
    let mut result = Vec::new();
    let mut cur = tails.last().copied();
    while let Some(idx) = cur {
        result.push(pairs[idx]);
        cur = predecessors[idx];
    }
    result.reverse();
    result
}

/// Convert a mapping (current→base) into per-line diff status.
fn build_diff_status(
    base: &[u64],
    current: &[u64],
    mapping: &[Option<usize>],
) -> Vec<LineDiffStatus> {
    let mut status = vec![LineDiffStatus::Added; current.len()];
    for (j, m) in mapping.iter().enumerate() {
        if m.is_some() {
            status[j] = LineDiffStatus::Unchanged;
        }
    }

    // Detect modifications and deletions in unmatched regions
    let mut cj = 0;
    while cj < current.len() {
        if status[cj] != LineDiffStatus::Added {
            cj += 1;
            continue;
        }
        let region_start = cj;
        while cj < current.len() && status[cj] == LineDiffStatus::Added {
            cj += 1;
        }
        let current_unmatched = cj - region_start;

        let base_start = if region_start > 0 {
            mapping[region_start - 1].map(|bi| bi + 1).unwrap_or(0)
        } else {
            0
        };
        let base_end = if cj < current.len() {
            mapping[cj].unwrap_or(base.len())
        } else {
            base.len()
        };
        let base_unmatched = base_end.saturating_sub(base_start);

        let modified_count = current_unmatched.min(base_unmatched);
        for k in 0..modified_count {
            status[region_start + k] = LineDiffStatus::Modified;
        }

        if base_unmatched > current_unmatched
            && cj < status.len()
            && status[cj] == LineDiffStatus::Unchanged
        {
            status[cj] = LineDiffStatus::DeletedAbove;
        }
    }

    status
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn is_indent_opener(ch: char) -> bool {
    matches!(ch, '{' | '(' | '[')
}

fn matching_closer(opener: char) -> Option<char> {
    match opener {
        '{' => Some('}'),
        '(' => Some(')'),
        '[' => Some(']'),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests/buffer.rs"]
mod tests;
