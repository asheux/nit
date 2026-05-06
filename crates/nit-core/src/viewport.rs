//! Editor viewport — line/col offsets and visible dimensions, plus the
//! `ensure_visible` cursor-following invariant.

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Viewport {
    pub offset_line: usize,
    pub offset_col: usize,
    pub height: usize,
    pub width: usize,
}

impl Viewport {
    pub fn with_dims(height: usize, width: usize) -> Self {
        Self {
            height,
            width,
            ..Default::default()
        }
    }

    pub fn ensure_visible(&mut self, cursor_line: usize, cursor_col: usize) {
        if cursor_line < self.offset_line {
            self.offset_line = cursor_line;
        } else if cursor_line >= self.offset_line + self.height.saturating_sub(1) {
            self.offset_line = cursor_line.saturating_sub(self.height.saturating_sub(1));
        }
        if self.width == 0 {
            return;
        }
        if cursor_col < self.offset_col {
            self.offset_col = cursor_col;
        } else if cursor_col >= self.offset_col + self.width.saturating_sub(1) {
            self.offset_col = cursor_col.saturating_sub(self.width.saturating_sub(1));
        }
    }
}
