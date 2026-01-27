use crate::{cursor::Cursor, viewport::Viewport};
use ropey::Rope;
use std::path::PathBuf;
use unicode_segmentation::UnicodeSegmentation;

const UNDO_LIMIT: usize = 256;

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
}

impl Buffer {
    pub fn new(name: impl Into<String>, content: Rope, path: Option<PathBuf>) -> Self {
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

    pub fn append(&mut self) {
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

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
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
        let insert_at = self.rope.line_to_char(line) + self.line_char_len(line);
        let mut text = String::from("\n");
        text.push_str(&indent);
        self.record_insert(insert_at, &text);
        self.rope.insert(insert_at, &text);
        self.cursor.line = line + 1;
        self.cursor.col = indent.chars().count();
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

    pub fn exit_insert_mode(&mut self) {
        self.end_edit_group();
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

    pub fn move_word_end(&mut self) {
        let len = self.rope.len_chars();
        if len == 0 {
            return;
        }
        let mut idx = self.char_index();
        if idx >= len {
            return;
        }
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if is_word(self.rope.char(idx)) {
            if idx + 1 < len && !is_word(self.rope.char(idx + 1)) {
                idx += 1;
            }
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
        let idx = self.char_index();
        self.record_insert(idx, "\n");
        self.begin_insert_group(idx);
        self.rope.insert_char(idx, '\n');
        self.cursor.line += 1;
        self.cursor.col = 0;
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
    }

    pub fn undo(&mut self) -> bool {
        if let Some(snapshot) = self.undo.pop() {
            self.end_edit_group();
            self.push_redo();
            self.rope = snapshot.rope;
            self.cursor = snapshot.cursor;
            self.dirty = snapshot.dirty;
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
