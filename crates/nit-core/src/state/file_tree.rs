use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileTreeKind {
    File,
    Dir,
    Loading,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirEntryModel {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileTreeRow {
    pub text: String,
    pub path: PathBuf,
    pub kind: FileTreeKind,
    pub depth: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileTreeState {
    pub open: bool,
    pub root: PathBuf,
    pub selected: usize,
    pub scroll_offset: usize,
    pub show_hidden: bool,
    pub show_ignored: bool,
    #[serde(skip)]
    pub rows: Vec<FileTreeRow>,
    #[serde(skip)]
    pub expanded_dirs: HashSet<PathBuf>,
    #[serde(skip)]
    pub loading_dirs: HashSet<PathBuf>,
    #[serde(skip)]
    pub cache: HashMap<PathBuf, Vec<DirEntryModel>>,
}
