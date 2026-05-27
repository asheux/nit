/// Buffer cursor in (line, char-column) coordinates. `col` indexes into the
/// line's char stream — not bytes, not graphemes — and the buffer clamps it
/// against `line_char_len(line)` (which excludes the trailing newline) on
/// every motion.
///
/// `desired_col` is vim's `curswant`: vertical motions (j/k, page-up/down,
/// Ctrl-d/Ctrl-u, gg/G) clamp the cursor to `min(desired_col, line_len)` on
/// the new line WITHOUT mutating `desired_col`. Horizontal motions clear it
/// so the next vertical move uses the current column as the new anchor. The
/// invariant: scrolling past a short line back to a long one restores the
/// original column instead of permanently truncating it.
#[derive(
    Copy, Clone, Debug, Default, Hash, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
    #[serde(default)]
    pub desired_col: Option<usize>,
}

impl Cursor {
    pub fn new(line: usize, col: usize) -> Self {
        Self {
            line,
            col,
            desired_col: None,
        }
    }
}
