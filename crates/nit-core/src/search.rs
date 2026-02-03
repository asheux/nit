use std::path::PathBuf;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SearchMode {
    Files,
    Content,
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Files
    }
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    #[serde(skip)]
    pub generation: u64,
    #[serde(skip)]
    pub file_results: Vec<SearchResultFile>,
    #[serde(skip)]
    pub match_results: Vec<SearchResultMatch>,
}

impl Default for FuzzySearchState {
    fn default() -> Self {
        Self {
            open: false,
            mode: SearchMode::Files,
            root: PathBuf::new(),
            query: String::new(),
            selected: 0,
            scroll_offset: 0,
            show_hidden: false,
            show_ignored: false,
            indexing: false,
            searching: false,
            status_msg: String::new(),
            generation: 0,
            file_results: Vec::new(),
            match_results: Vec::new(),
        }
    }
}

impl FuzzySearchState {
    pub fn open(&mut self, mode: SearchMode, root: PathBuf) {
        self.open = true;
        self.mode = mode;
        self.root = root;
        self.query.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.status_msg = "Indexing…".into();
        self.file_results.clear();
        self.match_results.clear();
        self.indexing = true;
        self.searching = false;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.selected = 0;
        self.scroll_offset = 0;
        self.indexing = false;
        self.searching = false;
        self.status_msg.clear();
        self.file_results.clear();
        self.match_results.clear();
        self.generation = self.generation.wrapping_add(1);
    }
}
