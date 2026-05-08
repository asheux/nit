/// Buffer cursor in (line, char-column) coordinates. `col` indexes into the
/// line's char stream — not bytes, not graphemes — and the buffer clamps it
/// against `line_char_len(line)` (which excludes the trailing newline) on
/// every motion.
#[derive(
    Copy, Clone, Debug, Default, Hash, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
}

impl Cursor {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}
