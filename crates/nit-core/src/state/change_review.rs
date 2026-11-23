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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewCommit {
    pub id: String,
    pub subject: String,
    pub author: String,
    pub date: String,
}

#[derive(Clone, Debug, Default)]
pub struct ChangeReview {
    pub root: PathBuf,
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub upstream_gone: bool,
    pub detached: bool,
    pub unborn: bool,
    pub commits: Vec<ReviewCommit>,
    pub preview_commit: Option<String>,
    pub history_patch_open: bool,
    pub files: Vec<ChangedFile>,
    pub selected: usize,
    pub scroll: usize,
    pub horizontal_scroll: u16,
    pub preview_generation: u64,
    pub preview: Vec<String>,
    pub preview_path: Option<PathBuf>,
    pub message: Option<String>,
    pub truncated: bool,
    pub refresh: bool,
}
