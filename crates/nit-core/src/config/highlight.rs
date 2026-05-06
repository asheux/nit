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
