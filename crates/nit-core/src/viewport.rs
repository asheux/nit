//! Editor viewport — line/col offsets, visible dimensions, and the
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

    /// Pull `offset_line` / `offset_col` toward the cursor so
    /// `(cursor_line, cursor_col)` lands inside the visible rect. The cursor
    /// tracks the *last visible* row/column (`offset + span - 1`) — vim's
    /// "scrolloff = 0" behavior — rather than the past-the-end edge.
    pub fn ensure_visible(&mut self, cursor_line: usize, cursor_col: usize) {
        self.offset_line = clamp_offset(self.offset_line, cursor_line, self.height);
        if self.width != 0 {
            self.offset_col = clamp_offset(self.offset_col, cursor_col, self.width);
        }
    }
}

fn clamp_offset(offset: usize, cursor: usize, span: usize) -> usize {
    if cursor < offset {
        return cursor;
    }
    let last_visible = span.saturating_sub(1);
    if cursor >= offset + last_visible {
        return cursor.saturating_sub(last_visible);
    }
    offset
}
