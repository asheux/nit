use std::path::PathBuf;

#[derive(
    Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum SearchMode {
    #[default]
    Files,
    Content,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchResultFile {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub score: i64,
    #[serde(skip)]
    pub matched_indices: Vec<usize>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchResultMatch {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub snippet: String,
    pub match_start: usize,
    pub match_len: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct FuzzySearchState {
    pub open: bool,
    pub mode: SearchMode,
    pub root: PathBuf,
    pub query: String,
    pub selected: usize,
    pub scroll_offset: usize,
    pub show_hidden: bool,
    pub show_ignored: bool,
    pub indexing: bool,
    pub searching: bool,
    pub status_msg: String,
    /// Bumped on every open/close so async runners can detect a stale
    /// invocation and discard their pending results.
    #[serde(skip)]
    pub generation: u64,
    #[serde(skip)]
    pub file_results: Vec<SearchResultFile>,
    #[serde(skip)]
    pub match_results: Vec<SearchResultMatch>,
}

impl FuzzySearchState {
    pub fn open(&mut self, mode: SearchMode, root: PathBuf) {
        self.reset_transient();
        self.open = true;
        self.mode = mode;
        self.root = root;
        self.query.clear();
        self.status_msg = "Indexing…".into();
        self.indexing = true;
    }

    pub fn close(&mut self) {
        self.reset_transient();
        self.open = false;
        self.status_msg.clear();
    }

    fn reset_transient(&mut self) {
        self.selected = 0;
        self.scroll_offset = 0;
        self.indexing = false;
        self.searching = false;
        self.file_results.clear();
        self.match_results.clear();
        self.generation = self.generation.wrapping_add(1);
    }
}
