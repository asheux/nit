use std::fmt;
use std::path::{Path, PathBuf};

use super::{is_excluded_directory, is_trackable_source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Descendable,
    Trackable,
    Irrelevant,
}

impl EntryKind {
    fn classify(entry_path: &Path) -> Self {
        if entry_path.is_dir() {
            Self::Descendable
        } else if is_trackable_source(entry_path) {
            Self::Trackable
        } else {
            Self::Irrelevant
        }
    }
}

impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Descendable => write!(f, "descendable"),
            Self::Trackable => write!(f, "trackable"),
            Self::Irrelevant => write!(f, "irrelevant"),
        }
    }
}

/// Depth-first walker over a directory tree. Iterates only the files
/// `is_trackable_source` recognises, skipping `is_excluded_directory`
/// branches. Caller owns the gitignore directory list; the walker
/// doesn't refresh it mid-iteration.
pub(super) struct SourceTreeWalker {
    pending_paths: Vec<PathBuf>,
    gitignored: Vec<String>,
}

impl SourceTreeWalker {
    pub(super) fn rooted_at(start: &Path, gitignored: Vec<String>) -> Self {
        Self {
            pending_paths: vec![start.to_path_buf()],
            gitignored,
        }
    }

    fn expand_directory(&mut self, parent: &Path) {
        let dir_name = parent
            .file_name()
            .and_then(|segment| segment.to_str())
            .unwrap_or("");

        if is_excluded_directory(dir_name, &self.gitignored) {
            return;
        }

        let Ok(listing) = std::fs::read_dir(parent) else {
            return;
        };

        for child in listing.flatten() {
            self.pending_paths.push(child.path());
        }
    }
}

impl Iterator for SourceTreeWalker {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(candidate) = self.pending_paths.pop() {
            match EntryKind::classify(&candidate) {
                EntryKind::Descendable => self.expand_directory(&candidate),
                EntryKind::Trackable => return Some(candidate),
                EntryKind::Irrelevant => {}
            }
        }
        None
    }
}
