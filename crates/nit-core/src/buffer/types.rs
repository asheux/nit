use ropey::Rope;

use crate::cursor::Cursor;

/// Diff state of a buffer line versus the on-disk base content. Drives the
/// gutter glyphs in the editor view.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LineDiffStatus {
    Unchanged,
    Added,
    Modified,
    /// One or more lines were deleted immediately above this line — the
    /// renderer paints a single marker on the surviving line below.
    DeletedAbove,
}

/// Tree-sitter-shaped (row, column) coordinate. `row` is 0-based; `column` is
/// a UTF-16 code-unit offset into the row.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BufferPoint {
    pub row: usize,
    pub column: usize,
}

/// One incremental edit feeding [`tree_sitter::InputEdit`]. Recorded on every
/// [`super::Buffer`] mutation and drained by the syntax engine.
#[derive(Clone, Debug)]
pub struct BufferEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: BufferPoint,
    pub old_end_point: BufferPoint,
    pub new_end_point: BufferPoint,
}

/// Kind of the most-recent edit, used to coalesce contiguous typing into a
/// single undo group. Only `Insert` is currently tracked; the enum exists so
/// future delete-grouping can be added without re-shaping `EditMeta`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    Insert,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct EditMeta {
    pub kind: EditKind,
    pub cursor_index: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Snapshot {
    pub rope: Rope,
    pub cursor: Cursor,
    pub dirty: bool,
}
