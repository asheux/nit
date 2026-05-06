use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct RulePickerState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ProtocolPickerState {
    pub open: bool,
    pub selected: usize,
    pub custom_input: String,
    pub custom_error: Option<String>,
    pub custom_preview: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileTreeKind {
    File,
    Dir,
    Loading,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DirEntryModel {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileTreeRow {
    pub text: String,
    pub path: PathBuf,
    pub kind: FileTreeKind,
    pub depth: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
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
