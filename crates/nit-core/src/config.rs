#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HighlightConfig {
    pub enabled: bool,
    pub engine: HighlightEngine,
    pub debounce_ms: u64,
    pub max_file_bytes: usize,
    pub max_spans_per_line: usize,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum HighlightEngine {
    TreeSitter,
    Plain,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EditorConfig {
    pub tab_width: u8,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub highlight: HighlightConfig,
    pub editor: EditorConfig,
}

impl Default for HighlightConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: HighlightEngine::TreeSitter,
            debounce_ms: 50,
            max_file_bytes: 2_000_000,
            max_spans_per_line: 256,
        }
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self { tab_width: 4 }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            highlight: HighlightConfig::default(),
            editor: EditorConfig::default(),
        }
    }
}
