use super::Buffer;

impl Buffer {
    pub fn ensure_visible(&mut self) {
        self.viewport
            .ensure_visible(self.cursor.line, self.cursor.col);
    }

    /// vim `Ctrl-d`: scroll half a screen down and move the cursor with it.
    /// Honours `desired_col` so scrolling through a stretch of short lines
    /// and back to longer ones restores the original column instead of
    /// stranding the cursor near col 0.
    pub fn scroll_half_page_down(&mut self) {
        let half = self.half_viewport();
        let max_line = self.last_content_line();
        let prev_line = self.cursor.line;
        let next_line = prev_line.saturating_add(half).min(max_line);
        if next_line != prev_line {
            self.end_edit_group();
        }
        self.cursor.line = next_line;
        self.viewport.offset_line = self.viewport.offset_line.saturating_add(half).min(max_line);
        self.apply_desired_col();
    }

    /// vim `Ctrl-u`: scroll half a screen up and move the cursor with it.
    /// Same `desired_col` invariant as the downward counterpart.
    pub fn scroll_half_page_up(&mut self) {
        let half = self.half_viewport();
        let prev_line = self.cursor.line;
        let next_line = prev_line.saturating_sub(half);
        if next_line != prev_line {
            self.end_edit_group();
        }
        self.cursor.line = next_line;
        self.viewport.offset_line = self.viewport.offset_line.saturating_sub(half);
        self.apply_desired_col();
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

    /// Last logical line index — like `len_lines() - 1`, but backs off the
    /// trailing empty "phantom" line that ropey produces when the buffer
    /// ends with `\n`. Vim's `Ctrl-D`/`Ctrl-U` stop on the last line that
    /// actually has content, not the synthetic empty trailer.
    fn last_content_line(&self) -> usize {
        let lines = self.rope.len_lines();
        let last = lines.saturating_sub(1);
        if last > 0 && self.line_char_len(last) == 0 {
            last - 1
        } else {
            last
        }
    }
}
