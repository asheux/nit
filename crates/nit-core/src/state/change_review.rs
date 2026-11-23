use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub index: char,
    pub worktree: char,
}

impl ChangedFile {
    /// Unmerged porcelain XY combinations include both-added and both-deleted.
    pub fn conflicted(&self) -> bool {
        self.index == 'U'
            || self.worktree == 'U'
            || matches!((self.index, self.worktree), ('A', 'A') | ('D', 'D'))
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChangeReview {
    pub root: PathBuf,
    pub branch: String,
    pub files: Vec<ChangedFile>,
    pub selected: usize,
    pub scroll: usize,
    pub preview: Vec<String>,
    pub preview_path: Option<PathBuf>,
    pub message: Option<String>,
    pub truncated: bool,
    pub refresh: bool,
}
