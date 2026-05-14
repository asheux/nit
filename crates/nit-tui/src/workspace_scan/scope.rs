//! Code-file predicates + adaptive concurrency cap for the workspace
//! genome scan. Kept out of `runtime` so other modules (file-watcher,
//! genome-retry) can pull in `is_code_file` without dragging the runtime
//! along.

use std::path::Path;
use std::thread::available_parallelism;

// Tree-sitter + GoL pegs a core per eval; past ~16 the contention for
// memory bandwidth swamps any speedup.
pub const WORKSPACE_SCAN_MAX_CAP: usize = 16;

// Floor for single/dual-core boxes — enough parallelism to hide I/O waits
// without starving the main loop.
pub const WORKSPACE_SCAN_MIN_CAP: usize = 4;

/// `available_parallelism() - 1` (reserve one core for the UI) clamped to
/// `[MIN_CAP, MAX_CAP]`. Falls back to `MIN_CAP` when the OS refuses to
/// report parallelism.
pub fn workspace_scan_max_in_flight() -> usize {
    available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(WORKSPACE_SCAN_MIN_CAP)
        .clamp(WORKSPACE_SCAN_MIN_CAP, WORKSPACE_SCAN_MAX_CAP)
}

/// True when the file's central-table entry has `is_code = true` — a
/// programming-language source worth scoring with the genome encoders.
/// Narrower than the file-watcher's `is_trackable_source` (which also
/// keeps markdown / json / toml for buffer-reload hooks) because
/// evaluating data files against the GoL simulation produces no signal.
pub fn is_code_file(path: &Path) -> bool {
    nit_core::languages::detect_by_path(path).is_some_and(|info| info.is_code)
}

/// State of a file in the workspace-scan pipeline; surfaces in the
/// gate-monitor LIVE sub-view.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceScanItemState {
    Queued,
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
