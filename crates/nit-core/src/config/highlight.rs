/// Syntax-highlight pipeline tuning.
///
/// Highlighting runs in a debounced background pass; the byte/span caps below
/// are guard rails for pathological files (multi-MB single-line minified JS,
/// generated lexer tables) that would otherwise lock the UI.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HighlightConfig {
    pub enabled: bool,
    pub engine: HighlightEngine,
    /// Wait this long after the last keystroke before re-highlighting; tuned
    /// to feel instant on typing bursts without re-parsing every char.
    pub debounce_ms: u64,
    /// Skip files larger than this — tree-sitter parse cost grows roughly
    /// linearly with input bytes, so very large files are intentionally
    /// rendered as plain text.
    pub max_file_bytes: usize,
    /// Per-line cap on highlight spans; protects the renderer from runaway
    /// span counts on files with extremely dense token boundaries.
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
