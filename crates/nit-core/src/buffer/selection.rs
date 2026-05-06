use super::Buffer;

impl Buffer {
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
}
