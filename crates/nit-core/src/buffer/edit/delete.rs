use super::super::cursor_motion::char_class;
use super::super::indent::pair_opener_closer;
use super::super::types::EditKind;
use super::super::Buffer;

impl Buffer {
    pub fn backspace(&mut self) {
        let idx = self.char_index();
        if idx == 0 {
            return;
        }
        if self.try_collapse_pair_at(idx) {
            return;
        }
        self.begin_delete_group(EditKind::DeleteBack, idx);
        if self.cursor.col > 0 {
            self.apply_rope_remove(idx - 1, idx);
            self.cursor.col -= 1;
        } else {
            // Joining the previous line — `idx > 0` already guarantees one exists.
            let prev_len = self.line_char_len(self.cursor.line - 1);
            self.apply_rope_remove(idx - 1, idx);
            self.cursor.line -= 1;
            self.cursor.col = prev_len;
        }
        self.finish_delete_group(EditKind::DeleteBack);
    }

    /// vim `db`: delete back to the start of the previous word/class run.
    pub fn delete_word_back(&mut self) -> String {
        self.end_edit_group();
        let end = self.char_index().min(self.rope.len_chars());
        if end == 0 {
            return String::new();
        }
        let start = self.scan_word_start_back();
        if start >= end {
            return String::new();
        }
        let removed = self.cut_range(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
        removed
    }

    /// vim `dB`: WORD-aware backward delete.
    pub fn delete_big_word_back(&mut self) -> String {
        self.end_edit_group();
        let end = self.char_index().min(self.rope.len_chars());
        if end == 0 {
            return String::new();
        }
        let start = self.scan_big_word_start_back();
        if start >= end {
            return String::new();
        }
        let removed = self.cut_range(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
        removed
    }

    pub fn delete_forward(&mut self) {
        let idx = self.char_index();
        let line_len = self.line_char_len(self.cursor.line);
        let on_char = self.cursor.col < line_len;
        let at_newline = self.cursor.line + 1 < self.rope.len_lines();
        if !(on_char || at_newline) {
            return;
        }
        self.begin_delete_group(EditKind::DeleteForward, idx);
        self.apply_rope_remove(idx, idx + 1);
        self.finish_delete_group(EditKind::DeleteForward);
    }

    /// vim `dw`: delete from cursor to the next word start (the same span
    /// `w` would traverse).
    pub fn delete_word_forward(&mut self) -> String {
        self.end_edit_group();
        let start = self.char_index().min(self.rope.len_chars());
        if start >= self.rope.len_chars() {
            return String::new();
        }
        let end = self.delete_word_forward_end(start);
        if end <= start {
            return String::new();
        }
        let removed = self.cut_range(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
        removed
    }

    /// vim `dW`: WORD-aware forward delete.
    pub fn delete_big_word_forward(&mut self) -> String {
        self.end_edit_group();
        let start = self.char_index().min(self.rope.len_chars());
        if start >= self.rope.len_chars() {
            return String::new();
        }
        let end = self.delete_big_word_forward_end(start);
        if end <= start {
            return String::new();
        }
        let removed = self.cut_range(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
        removed
    }

    /// vim `de`: delete to the end of the current/next word (inclusive).
    pub fn delete_word_end(&mut self) -> String {
        self.end_edit_group();
        let start = self.char_index().min(self.rope.len_chars());
        let Some(end_inclusive) = self.scan_word_end_forward() else {
            return String::new();
        };
        let end = (end_inclusive + 1).min(self.rope.len_chars());
        if end <= start {
            return String::new();
        }
        let removed = self.cut_range(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
        removed
    }

    /// vim `cw` (and `cW`): change to end of the current word run, *without*
    /// the "advance past a single-char word edge" jump that `e`/`de` perform.
    /// When the cursor sits on whitespace there's no in-word case to honour,
    /// so it falls back to `delete_word_forward` (the `w` motion). See
    /// `:h cw` — this is the documented quirk that distinguishes `cw` from
    /// `ce`/`dw`.
    pub fn delete_word_change(&mut self, big: bool) -> String {
        self.end_edit_group();
        let start = self.char_index().min(self.rope.len_chars());
        let len = self.rope.len_chars();
        if start >= len {
            return String::new();
        }
        let here = self.rope.char(start);
        if here.is_whitespace() {
            return if big {
                self.delete_big_word_forward()
            } else {
                self.delete_word_forward()
            };
        }
        // Walk to the end of the current class run (or to end-of-non-whitespace
        // for the WORD variant); do not advance to the next word.
        let cls = char_class(here);
        let mut end = start + 1;
        while end < len {
            let c = self.rope.char(end);
            let same = if big {
                !c.is_whitespace()
            } else {
                char_class(c) == cls
            };
            if !same {
                break;
            }
            end += 1;
        }
        let removed = self.cut_range(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
        removed
    }

    /// vim `dE`: WORD-aware delete to end of run.
    pub fn delete_big_word_end(&mut self) -> String {
        self.end_edit_group();
        let start = self.char_index().min(self.rope.len_chars());
        let Some(end_inclusive) = self.scan_big_word_end_forward() else {
            return String::new();
        };
        let end = (end_inclusive + 1).min(self.rope.len_chars());
        if end <= start {
            return String::new();
        }
        let removed = self.cut_range(start, end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clamp_col();
        removed
    }

    /// vim `dd`: remove the current line and return its content with a
    /// trailing newline (line-wise yank). The newline is always appended so a
    /// later `p` paste re-creates the row even when the source was the
    /// buffer's final newline-less line.
    pub fn delete_line(&mut self) -> String {
        self.end_edit_group();
        let total = self.rope.len_lines();
        if total == 0 {
            return String::new();
        }
        self.push_undo();
        let line = self.cursor.line.min(total - 1);
        let start = self.rope.line_to_char(line);
        let end = self.line_end_char_index(line);
        let mut removed = if end > start {
            let text = self.rope.slice(start..end).to_string();
            self.record_delete(start, end);
            self.rope.remove(start..end);
            self.dirty = true;
            text
        } else {
            String::new()
        };
        if !removed.ends_with('\n') {
            removed.push('\n');
        }
        let new_total = self.rope.len_lines();
        self.cursor.col = 0;
        if new_total == 0 {
            self.cursor.line = 0;
        } else if self.cursor.line >= new_total {
            self.cursor.line = new_total - 1;
        }
        self.clamp_col();
        removed
    }

    /// vim `D`: delete from cursor to end of line. Returns the removed text
    /// for char-wise yank wrapping at the caller.
    pub fn delete_to_end(&mut self) -> String {
        self.end_edit_group();
        let line = self.clamped_cursor_line();
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let col = self.cursor.col.min(line_len);
        let start = line_start + col;
        let end = line_start + line_len;
        if end <= start {
            return String::new();
        }
        let removed = self.rope.slice(start..end).to_string();
        self.push_undo();
        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.dirty = true;
        self.clamp_col();
        removed
    }

    /// vim `S` / `cc`: clear the current line, preserving leading indent.
    /// Caller is expected to switch to insert mode afterwards.
    pub fn substitute_line(&mut self) {
        self.end_edit_group();
        let line = self.clamped_cursor_line();
        let line_start = self.rope.line_to_char(line);
        let line_len = self.line_char_len(line);
        let indent_chars = self.line_indent(line).chars().count().min(line_len);
        if line_len > indent_chars {
            let start = line_start + indent_chars;
            let end = line_start + line_len;
            self.push_undo();
            self.record_delete(start, end);
            self.rope.remove(start..end);
            self.dirty = true;
        }
        self.cursor.col = indent_chars;
    }

    /// Pop `[start, end)` from the rope under a fresh undo entry and return
    /// the removed text. Used by every word/end-of-line delete that surfaces
    /// text for the unnamed yank register.
    fn cut_range(&mut self, start: usize, end: usize) -> String {
        if start >= end {
            return String::new();
        }
        let text = self.rope.slice(start..end).to_string();
        self.push_undo();
        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.dirty = true;
        text
    }

    /// Mutate the rope between `[start, end)` without touching the undo stack
    /// — the surrounding `begin_delete_group` / `finish_delete_group` pair
    /// decides when to snapshot so a contiguous run collapses into one entry.
    fn apply_rope_remove(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.dirty = true;
    }

    /// Auto-pair-aware backspace: when the cursor sits between an opener and
    /// its matching closer with nothing in between (the shape left after the
    /// auto-pair on `(`, `[`, `{`, `"`, `'`), delete both as one edit so a
    /// single `u` rewinds the pair atomically. Returns `true` when the pair
    /// was collapsed and the caller should stop processing.
    fn try_collapse_pair_at(&mut self, idx: usize) -> bool {
        let len = self.rope.len_chars();
        if idx == 0 || idx >= len {
            return false;
        }
        let opener = self.rope.char(idx - 1);
        let Some(closer) = pair_opener_closer(opener) else {
            return false;
        };
        if self.rope.char(idx) != closer {
            return false;
        }
        self.end_edit_group();
        self.push_undo();
        self.apply_rope_remove(idx - 1, idx + 1);
        self.cursor.col -= 1;
        true
    }

    fn line_end_char_index(&self, line: usize) -> usize {
        if line + 1 < self.rope.len_lines() {
            self.rope.line_to_char(line + 1)
        } else {
            self.rope.len_chars()
        }
    }

    /// Walk forward from `start` like `w` would, returning the end of the
    /// delete span. Diverges from `scan_word_start_forward` in the final
    /// clamp: a delete needs the exact past-the-end index, not the
    /// last-char-of-buffer fallback the motion uses.
    fn delete_word_forward_end(&self, start: usize) -> usize {
        let len = self.rope.len_chars();
        if start >= len {
            return start;
        }
        let cls = super::super::cursor_motion::char_class(self.rope.char(start));
        let mut idx = start;
        if cls != super::super::cursor_motion::CharClass::Whitespace {
            while idx < len && super::super::cursor_motion::char_class(self.rope.char(idx)) == cls {
                idx += 1;
            }
        }
        while idx < len && self.rope.char(idx).is_whitespace() {
            idx += 1;
        }
        idx
    }

    fn delete_big_word_forward_end(&self, start: usize) -> usize {
        let len = self.rope.len_chars();
        if start >= len {
            return start;
        }
        let mut idx = start;
        if !self.rope.char(idx).is_whitespace() {
            while idx < len && !self.rope.char(idx).is_whitespace() {
                idx += 1;
            }
        }
        while idx < len && self.rope.char(idx).is_whitespace() {
            idx += 1;
        }
        idx
    }
}
