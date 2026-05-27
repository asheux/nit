use super::undo_log::GroupHint;
use super::Buffer;

impl Buffer {
    pub fn set_selection_anchor(&mut self) {
        self.selection_anchor = Some(self.char_index());
    }

    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// Active selection as `(start, end)` char indices. The end is **exclusive
    /// for slicing but vim-inclusive for the visual cell**: it points one past
    /// the char under the cursor, so `rope.slice(start..end)` covers the cell.
    /// Returns `None` for an empty buffer or a degenerate range.
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor?;
        let cursor = self.char_index();
        let len = self.rope.len_chars();
        let (lo, hi) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        let end = (hi + 1).min(len);
        (lo < len && end > lo).then_some((lo, end))
    }

    pub fn yank_selection(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        Some(self.rope.slice(start..end).to_string())
    }

    /// Linewise yank ("Y" in vim) of the cursor's line. The result is always
    /// `\n`-terminated so a paste re-creates the trailing newline even when
    /// the source line was the buffer's last (newline-less) line.
    pub fn yank_line(&self) -> String {
        let line = self.clamped_cursor_line();
        let start = self.rope.line_to_char(line);
        let end = self.line_char_end(line);
        let mut text = self.rope.slice(start..end).to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }

    pub fn delete_selection(&mut self) -> bool {
        self.end_edit_group();
        let Some((start, end)) = self.selection_range() else {
            return false;
        };
        if start >= end {
            return false;
        }
        let text = self.rope.slice(start..end).to_string();
        let before = self.cursor;
        self.apply_selection_removal(start, end);
        self.dirty = true;
        self.record_delete_delta(start, &text, before, GroupHint::Atomic);
        true
    }

    pub(super) fn apply_selection_removal(&mut self, start: usize, end: usize) {
        self.record_delete(start, end);
        self.rope.remove(start..end);
        self.set_cursor_from_char_index(start.min(self.rope.len_chars()));
        self.clear_selection();
    }
}
