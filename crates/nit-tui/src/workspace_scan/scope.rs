//! Code-file scope predicates and concurrency caps for the workspace
//! genome scan. Kept separate from the runtime driver so the file-watcher
//! and genome-retry paths can pull in `is_code_file` without dragging in
//! the rest of `WorkspaceScanRuntime`.

use std::path::Path;
use std::thread::available_parallelism;

/// Absolute ceiling on concurrent scan threads. Each eval needs a 2 MB
/// stack and saturates a core during tree-sitter + GoL simulation; beyond
/// ~16 we start contending for memory bandwidth without meaningful speedup.
pub const WORKSPACE_SCAN_MAX_CAP: usize = 16;

/// Minimum concurrency for single/dual-core boxes — enough parallelism to
/// hide I/O waits without starving the main loop.
pub const WORKSPACE_SCAN_MIN_CAP: usize = 4;

/// Adaptive in-flight cap: `available_parallelism() - 1` (reserve one core
/// for the main loop) clamped to `[MIN_CAP, MAX_CAP]`. Falls back to
/// MIN_CAP when the OS refuses to report parallelism.
pub fn workspace_scan_max_in_flight() -> usize {
    available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(WORKSPACE_SCAN_MIN_CAP)
        .clamp(WORKSPACE_SCAN_MIN_CAP, WORKSPACE_SCAN_MAX_CAP)
}

/// Extensions the genome scan actually evaluates. Narrower than the
/// file-watcher's `SOURCE_EXTENSIONS` — markdown/toml/yaml/json/txt have
/// no tree-sitter parser for the genome computation, so evaluating them
/// is wasted work. Each extension costs ~100–500 ms of CPU per file.
pub const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "mjs", "cjs", "ts", "tsx", "go", "java", "kt", "scala", "cs", "rb",
    "c", "cpp", "h", "hpp", "swift", "sh", "bash", "zsh", "sql", "html", "htm", "css",
];

/// True for paths whose extension is in `CODE_EXTENSIONS`. The file-watcher
/// keeps its broader `is_trackable_source` for buffer-reload purposes; the
/// scan uses this narrower predicate so the background eval queue only
/// touches code.
pub fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|raw| raw.to_str())
        .is_some_and(|ext| CODE_EXTENSIONS.contains(&ext))
}

/// State of a file in the workspace-scan pipeline. Surfaces in the LIVE
/// sub-view of the gate monitor so the operator can see what's queued vs
/// what's actively being evaluated.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceScanItemState {
    /// File sits in `pending`, waiting for an in-flight slot.
    Queued,
    /// A worker thread is currently computing the report for this file.
    Evaluating,
}

impl WorkspaceScanItemState {
    pub fn label(self) -> &'static str {
        match self {
            WorkspaceScanItemState::Queued => "queued",
            WorkspaceScanItemState::Evaluating => "evaluating",
        }
    }
}
