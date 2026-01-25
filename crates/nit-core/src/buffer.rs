use crate::{cursor::Cursor, viewport::Viewport};
use ropey::Rope;
use std::path::PathBuf;
use unicode_segmentation::UnicodeSegmentation;

const UNDO_LIMIT: usize = 256;

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
    pub cursor: Cursor,
    pub viewport: Viewport,
    dirty: bool,
}

impl Buffer {
    pub fn new(name: impl Into<String>, content: Rope, path: Option<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path,
            rope: content,
            undo: Vec::new(),
            cursor: Cursor::default(),
            viewport: Viewport::default(),
            dirty: false,
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

    pub fn bytes_len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn lines_len(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn content_as_string(&self) -> String {
        self.rope.to_string()
    }

    pub fn grapheme_width_at_line(&self, line: usize) -> usize {
        let line_text = self.line_as_str(line);
        line_text.graphemes(true).count()
    }

    pub fn line_as_string(&self, line: usize) -> String {
        self.line_as_str(line)
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
            self.rope.line(line).chars().count()
        }
    }

    fn char_index(&self) -> usize {
        let line_start = self.rope.line_to_char(self.cursor.line);
        let col = self.cursor.col.min(self.line_char_len(self.cursor.line));
        line_start + col
    }

    fn clamp_col(&mut self) {
        if self.is_line_blank(self.cursor.line) {
            self.cursor.col = 0;
            return;
        }
        let len = self.line_char_len(self.cursor.line);
        if self.cursor.col > len {
            self.cursor.col = len;
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.line_char_len(self.cursor.line);
        }
    }

    pub fn move_right(&mut self) {
        let len = self.line_char_len(self.cursor.line);
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.rope.len_lines() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.clamp_col();
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.line + 1 < self.rope.len_lines() {
            self.cursor.line += 1;
            self.clamp_col();
        }
    }

    pub fn page_up(&mut self, count: usize) {
        let jump = count.min(self.cursor.line);
        self.cursor.line -= jump;
        self.clamp_col();
    }

    pub fn page_down(&mut self, count: usize) {
        let max_line = self.rope.len_lines().saturating_sub(1);
        self.cursor.line = (self.cursor.line + count).min(max_line);
        self.clamp_col();
    }

    pub fn move_home(&mut self) {
        self.cursor.col = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor.col = self.line_char_len(self.cursor.line);
    }

    pub fn exit_insert_mode(&mut self) {
        if self.is_line_blank(self.cursor.line) {
            self.cursor.col = 0;
        } else if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
    }

    pub fn go_to_top(&mut self) {
        self.cursor.line = 0;
        self.clamp_col();
    }

    pub fn go_to_bottom(&mut self) {
        let last = self.rope.len_lines().saturating_sub(1);
        self.cursor.line = last;
        self.clamp_col();
    }

    pub fn insert_char(&mut self, c: char) {
        self.push_undo();
        let idx = self.char_index();
        self.rope.insert_char(idx, c);
        self.cursor.col += 1;
        self.dirty = true;
    }

    pub fn insert_tab(&mut self) {
        self.insert_char('\t');
    }

    pub fn insert_newline(&mut self) {
        self.push_undo();
        let idx = self.char_index();
        self.rope.insert_char(idx, '\n');
        self.cursor.line += 1;
        self.cursor.col = 0;
        self.dirty = true;
    }

    pub fn backspace(&mut self) {
        if self.cursor.col > 0 {
            let idx = self.char_index();
            if idx > 0 {
                self.push_undo();
                self.rope.remove(idx - 1..idx);
                self.cursor.col -= 1;
                self.dirty = true;
            }
        } else if self.cursor.line > 0 {
            let prev_len = self.line_char_len(self.cursor.line - 1);
            let idx = self.char_index();
            if idx > 0 {
                self.push_undo();
                self.rope.remove(idx - 1..idx);
                self.cursor.line -= 1;
                self.cursor.col = prev_len;
                self.dirty = true;
            }
        }
    }

    pub fn delete_forward(&mut self) {
        let idx = self.char_index();
        let line_len = self.line_char_len(self.cursor.line);
        if self.cursor.col < line_len {
            self.push_undo();
            self.rope.remove(idx..idx + 1);
            self.dirty = true;
        } else if self.cursor.line + 1 < self.rope.len_lines() {
            // delete the newline
            self.push_undo();
            self.rope.remove(idx..idx + 1);
            self.dirty = true;
        }
    }

    pub fn ensure_visible(&mut self) {
        self.viewport.ensure_visible(self.cursor.line);
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn undo(&mut self) -> bool {
        if let Some(snapshot) = self.undo.pop() {
            self.rope = snapshot.rope;
            self.cursor = snapshot.cursor;
            self.dirty = snapshot.dirty;
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
    }

    fn is_line_blank(&self, line: usize) -> bool {
        if line >= self.rope.len_lines() {
            return true;
        }
        let text = self.rope.line(line).to_string();
        text.trim_end_matches('\n').trim().is_empty()
    }
}
