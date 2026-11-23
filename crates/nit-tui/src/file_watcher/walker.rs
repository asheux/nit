use std::path::{Path, PathBuf};

use super::{is_excluded_directory, is_trackable_source};

/// Only recurse into real directories. A repository can contain links to
/// ancestor directories, other checkouts and special devices.
pub(super) struct SourceTreeWalker {
    directories: Vec<PathBuf>,
    files: Vec<PathBuf>,
    gitignored: Vec<String>,
}

impl SourceTreeWalker {
    pub(super) fn rooted_at(start: &Path, gitignored: Vec<String>) -> Self {
        Self {
            directories: vec![start.to_path_buf()],
            files: Vec::new(),
            gitignored,
        }
    }
}

impl Iterator for SourceTreeWalker {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(file) = self.files.pop() {
                return Some(file);
            }
            let directory = self.directories.pop()?;
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if !is_excluded_directory(
                        &entry.file_name().to_string_lossy(),
                        &self.gitignored,
                    ) {
                        self.directories.push(entry.path());
                    }
                } else if kind.is_file() && is_trackable_source(&entry.path()) {
                    self.files.push(entry.path());
                }
            }
        }
    }
}
