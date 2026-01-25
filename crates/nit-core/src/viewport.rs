#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Viewport {
    pub offset_line: usize,
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

    pub fn ensure_visible(&mut self, cursor_line: usize) {
        if cursor_line < self.offset_line {
            self.offset_line = cursor_line;
        } else if cursor_line >= self.offset_line + self.height.saturating_sub(1) {
            self.offset_line = cursor_line.saturating_sub(self.height.saturating_sub(1));
        }
    }
}
