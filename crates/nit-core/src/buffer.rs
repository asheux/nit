use ropey::Rope;
use std::path::PathBuf;
use unicode_segmentation::UnicodeSegmentation;

use crate::cursor::Cursor;
use crate::viewport::Viewport;

mod cursor_motion;
mod diff;
mod edit;
mod edit_tracking;
mod indent;
mod scroll;
mod search;
mod selection;
mod types;
mod undo;

pub use types::{BufferEdit, BufferPoint, LineDiffStatus};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Buffer {
    name: String,
    path: Option<PathBuf>,
    #[serde(skip)]
    rope: Rope,
    #[serde(skip)]
    undo: Vec<types::Snapshot>,
    #[serde(skip)]
    redo: Vec<types::Snapshot>,
    #[serde(skip)]
    last_edit: Option<types::EditMeta>,
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
        let base_hashes = diff::rope_line_hashes(&content);
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
        self.line_as_str(line).graphemes(true).count()
    }

    pub fn line_as_string(&self, line: usize) -> String {
        self.line_as_str(line)
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

    pub(super) fn line_char_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            0
        } else {
            let slice = self.rope.line(line);
            let mut len = slice.len_chars();
            // Strip trailing newline so col == 0..line_char_len matches
            // the visible character width (vim semantics).
            if len > 0 && slice.chars().last() == Some('\n') {
                len = len.saturating_sub(1);
            }
            len
        }
    }

    pub(super) fn char_index(&self) -> usize {
        let line_start = self.rope.line_to_char(self.cursor.line);
        let col = self.cursor.col.min(self.line_char_len(self.cursor.line));
        line_start + col
    }

    pub(super) fn clamp_col(&mut self) {
        let len = self.line_char_len(self.cursor.line);
        // Strict `>` not `>=`: cursor may sit one past last char during `append`.
        if self.cursor.col > len {
            self.cursor.col = len;
        }
    }

    pub(super) fn is_line_blank(&self, line: usize) -> bool {
        if line >= self.rope.len_lines() {
            return true;
        }
        let text = self.rope.line(line).to_string();
        text.trim_end_matches('\n').trim().is_empty()
    }
}

#[cfg(test)]
#[path = "tests/buffer.rs"]
mod tests;
