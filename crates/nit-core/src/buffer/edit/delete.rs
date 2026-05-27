use super::super::cursor_motion::char_class;
use super::super::indent::pair_opener_closer;
use super::super::undo_log::GroupHint;
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
        if self.cursor.line > 0 && self.is_line_blank(self.cursor.line) {
            self.collapse_blank_line_back();
            return;
        }
        let before = self.cursor;
        let text = self.rope.slice(idx - 1..idx).to_string();
        if self.cursor.col > 0 {
            self.apply_rope_remove(idx - 1, idx);
            self.cursor.col -= 1;
        } else {
            let prev_len = self.line_char_len(self.cursor.line - 1);
            self.apply_rope_remove(idx - 1, idx);
            self.cursor.line -= 1;
            self.cursor.col = prev_len;
        }
        self.record_delete_delta(idx - 1, &text, before, GroupHint::DeleteBack);
    }

    /// T8b smart-delete: cursor sits on a whitespace-only line, so pull the
    /// whole row plus its leading newline in one shot. Routes through the
    /// standard delta recorder so T7's undo log captures it as one
    /// transaction — no per-space backspace events spam the log or the UI.
    fn collapse_blank_line_back(&mut self) {
        let before = self.cursor;
        let line = self.cursor.line;
        let line_start = self.rope.line_to_char(line);
        let line_content_len = self.line_char_len(line);
        let start = line_start - 1;
        let end = line_start + line_content_len;
        let text = self.rope.slice(start..end).to_string();
        let prev_len = self.line_char_len(line - 1);
        self.apply_rope_remove(start, end);
        self.cursor.line = line - 1;
        self.cursor.col = prev_len;
        self.record_delete_delta(start, &text, before, GroupHint::Atomic);
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
        let before = self.cursor;
        let text = self.rope.slice(idx..idx + 1).to_string();
        self.apply_rope_remove(idx, idx + 1);
        self.record_delete_delta(idx, &text, before, GroupHint::DeleteForward);
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
    /// so it falls back to `delete_word_forward` (the `w` motion).
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
        let line = self.cursor.line.min(total - 1);
        let start = self.rope.line_to_char(line);
        let end = self.line_end_char_index(line);
        let mut removed = if end > start {
            let text = self.rope.slice(start..end).to_string();
            let before = self.cursor;
            self.record_delete(start, end);
            self.rope.remove(start..end);
            self.dirty = true;
            self.record_delete_delta(start, &text, before, GroupHint::Atomic);
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

    /// vim `D`: delete from cursor to end of line.
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
        let before = self.cursor;
        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.dirty = true;
        self.clamp_col();
        self.record_delete_delta(start, &removed, before, GroupHint::Atomic);
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
            let text = self.rope.slice(start..end).to_string();
            let before = self.cursor;
            self.record_delete(start, end);
            self.rope.remove(start..end);
            self.dirty = true;
            self.record_delete_delta(start, &text, before, GroupHint::Atomic);
        }
        self.cursor.col = indent_chars;
    }

    /// Pop `[start, end)` from the rope as one atomic transaction. Used by
    /// every word/end-of-line delete that surfaces text for the unnamed yank
    /// register.
    fn cut_range(&mut self, start: usize, end: usize) -> String {
        if start >= end {
            return String::new();
        }
        let text = self.rope.slice(start..end).to_string();
        let before = self.cursor;
        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.dirty = true;
        self.record_delete_delta(start, &text, before, GroupHint::Atomic);
        text
    }

    /// Mutate the rope between `[start, end)` without touching the undo log —
    /// the surrounding `record_delete_delta` call decides how to group the
    /// edit so a contiguous backspace run collapses into one entry.
    fn apply_rope_remove(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.dirty = true;
    }

    /// Auto-pair-aware backspace: when the cursor sits between an opener and
    /// its matching closer with nothing in between, delete both as one edit
    /// so a single `u` rewinds the pair atomically.
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
        let text = self.rope.slice(idx - 1..idx + 1).to_string();
        let before = self.cursor;
        self.end_edit_group();
        self.apply_rope_remove(idx - 1, idx + 1);
        self.cursor.col -= 1;
        self.record_delete_delta(idx - 1, &text, before, GroupHint::Atomic);
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
