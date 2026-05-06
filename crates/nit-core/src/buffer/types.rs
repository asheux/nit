use ropey::Rope;

use crate::cursor::Cursor;

/// Per-line diff status relative to the base (on-disk) content.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LineDiffStatus {
    Unchanged,
    Added,
    Modified,
    /// One or more lines were deleted just above this line.
    DeletedAbove,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BufferPoint {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
pub struct BufferEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: BufferPoint,
    pub old_end_point: BufferPoint,
    pub new_end_point: BufferPoint,
}

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
