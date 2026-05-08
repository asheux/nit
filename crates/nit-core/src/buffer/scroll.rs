use super::Buffer;

impl Buffer {
    pub fn ensure_visible(&mut self) {
        self.viewport
            .ensure_visible(self.cursor.line, self.cursor.col);
    }

    /// vim `Ctrl-d`: scroll half a screen down and move the cursor with it.
    pub fn scroll_half_page_down(&mut self) {
        self.end_edit_group();
        let half = self.half_viewport();
        let max_line = self.rope.len_lines().saturating_sub(1);
        self.cursor.line = self.cursor.line.saturating_add(half).min(max_line);
        self.viewport.offset_line = self.viewport.offset_line.saturating_add(half).min(max_line);
        self.clamp_col();
    }

    /// vim `Ctrl-u`: scroll half a screen up and move the cursor with it.
    pub fn scroll_half_page_up(&mut self) {
        self.end_edit_group();
        let half = self.half_viewport();
        self.cursor.line = self.cursor.line.saturating_sub(half);
        self.viewport.offset_line = self.viewport.offset_line.saturating_sub(half);
        self.clamp_col();
    }

    /// vim `zz`: center the viewport on the cursor line.
    pub fn center_viewport_on_cursor(&mut self) {
        self.viewport.offset_line = self.cursor.line.saturating_sub(self.half_viewport());
    }

    /// vim `zt`: scroll so the cursor line is at the top of the viewport.
    pub fn viewport_top_on_cursor(&mut self) {
        self.viewport.offset_line = self.cursor.line;
    }

    /// vim `zb`: scroll so the cursor line is at the bottom of the viewport.
    pub fn viewport_bottom_on_cursor(&mut self) {
        let last_visible_row = self.viewport.height.max(1).saturating_sub(1);
        self.viewport.offset_line = self.cursor.line.saturating_sub(last_visible_row);
    }

    fn half_viewport(&self) -> usize {
        (self.viewport.height.max(1) / 2).max(1)
    }
}
