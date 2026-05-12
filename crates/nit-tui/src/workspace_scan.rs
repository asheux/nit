//! Workspace-wide genome scan driver.
//!
//! At launch the runtime hydrates `state.genome_reports` from the on-disk
//! cache in `.nit/genome/`, purges entries whose files no longer exist, then
//! walks the workspace to enqueue missing or stale reports. File-watcher
//! events feed the same pending queue so cached reports stay in sync with
//! the filesystem without gating agent dispatch.
//!
//! The runtime is non-blocking: the walk runs once at hydration and every
//! eval spawns a short-lived worker thread (`GenomeWorker`). The in-flight
//! cap adapts to `available_parallelism()` so nit uses the machine's cores
//! without freezing the UI.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;
use std::time::UNIX_EPOCH;

use nit_core::AppState;

use crate::file_watcher::{is_excluded_directory, walk_source_files, IGNORED_DIRS};
use crate::genome_worker::GenomeWorker;

/// Absolute ceiling on concurrent scan threads. Each eval needs a 2 MB
/// stack and saturates a core during tree-sitter + GoL simulation; beyond
/// ~16 we start contending for memory bandwidth without meaningful speedup.
const WORKSPACE_SCAN_MAX_CAP: usize = 16;

/// Minimum concurrency. On single/dual-core boxes we still want enough
/// parallelism to hide I/O waits.
const WORKSPACE_SCAN_MIN_CAP: usize = 4;

/// Returns the adaptive in-flight cap for the workspace scan. Uses
/// `available_parallelism() - 1` (reserve one core for the main loop)
/// clamped to `[WORKSPACE_SCAN_MIN_CAP, WORKSPACE_SCAN_MAX_CAP]`. Falls back
/// to `WORKSPACE_SCAN_MIN_CAP` when the OS refuses to report.
pub fn workspace_scan_max_in_flight() -> usize {
    available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(WORKSPACE_SCAN_MIN_CAP)
        .clamp(WORKSPACE_SCAN_MIN_CAP, WORKSPACE_SCAN_MAX_CAP)
}

/// Extensions the genome scan actually evaluates. This is a narrower subset
/// of `file_watcher::SOURCE_EXTENSIONS` — the genome computation is only
/// meaningful for code, so evaluating markdown/toml/yaml/json/txt is wasted
/// work (tree-sitter has no parser for most of them and the GoL landscape
/// for a README isn't actionable). Keep this list tight: every extension
/// here costs ~100–500 ms of CPU per file on the background scan.
pub const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "mjs", "cjs", "ts", "tsx", "go", "java", "kt", "scala", "cs", "rb",
    "c", "cpp", "h", "hpp", "swift", "sh", "bash", "zsh", "sql", "html", "htm", "css",
];

/// True for paths whose extension is in `CODE_EXTENSIONS`. File-watcher's
/// broader `is_trackable_source` stays intact for buffer-reload purposes
/// (which legitimately care about markdown/config changes); the scan uses
/// this narrower predicate so the background eval queue only touches code.
pub fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|raw| raw.to_str())
        .is_some_and(|ext| CODE_EXTENSIONS.contains(&ext))
}

/// State of a file in the workspace-scan pipeline. Surfaces in the LIVE
/// sub-view of the gate monitor so the operator can see what's being
/// evaluated right now vs what's queued.
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

/// Driver for the background genome scan. One instance lives in `run_loop`
/// alongside `GenomeWorker`; the main loop calls `hydrate` once at startup,
/// `drive` every tick, and `note_completed` when a workspace-scan result
/// lands on the worker's receiver.
pub struct WorkspaceScanRuntime {
    pending: VecDeque<PathBuf>,
    /// Paths currently being evaluated by the genome worker (dedup guard so
    /// a `drive` tick never re-dispatches the same path while its thread is
    /// still running).
    dispatched: HashSet<PathBuf>,
    /// Total files the scan has ever queued (hydrate seed + deletions that
    /// happen to need re-queue of nothing). Used as the denominator in the
    /// UI progress line.
    total: usize,
    /// Files whose result has landed. Resets to zero when a new scan is
    /// seeded (e.g. initial hydration); incremented on every `note_completed`.
    done: usize,
    /// Guards against re-running the launch walk. Session-lifetime flag —
    /// once hydrated, we don't re-walk the workspace.
    hydrated: bool,
    /// Max concurrent eval threads — computed once at construction so tests
    /// can override via `with_max_in_flight` without touching globals.
    max_in_flight: usize,
}

impl Default for WorkspaceScanRuntime {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            dispatched: HashSet::new(),
            total: 0,
            done: 0,
            hydrated: false,
            max_in_flight: workspace_scan_max_in_flight(),
        }
    }
}

impl WorkspaceScanRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn with_max_in_flight(max_in_flight: usize) -> Self {
        Self {
            max_in_flight: max_in_flight.max(1),
            ..Self::default()
        }
    }

    /// Launch-time hydration. Reads any cached genome reports from
    /// `.nit/genome/`, drops entries whose files no longer exist (including
    /// the on-disk JSON), then walks the workspace for source files and
    /// enqueues anything missing or stale against `GenomeReport::timestamp_ms`.
    ///
    /// Idempotent: subsequent calls are no-ops. Silent no-op when
    /// `genome_context_enabled` is off — no reports are loaded and nothing
    /// is scanned.
    pub fn hydrate(&mut self, state: &mut AppState) {
        if self.hydrated {
            return;
        }
        self.hydrated = true;

        if !state.settings.genome.genome_context_enabled {
            return;
        }

        let workspace_root = state.workspace_root.clone();

        // 1. Load cache from disk into state.genome_reports.
        let mut loaded = nit_core::agent_bus::load_genome_reports(&workspace_root);

        // 2. Purge entries whose files no longer exist (renamed/deleted while
        //    nit was closed). Deleting the cache file keeps the on-disk state
        //    honest across launches.
        let stale: Vec<PathBuf> = loaded
            .keys()
            .filter(|path| !path.exists())
            .cloned()
            .collect();
        for path in stale {
            loaded.remove(&path);
            nit_core::agent_bus::delete_genome_report(&workspace_root, &path);
        }

        // Sweep legacy layout + enforce size caps once per launch.
        nit_core::agent_bus::gc_genome_cache(&workspace_root);

        // Merge into state. The map is cleared at startup so no conflicts
        // exist, but `insert` honors the newest report on follow-up sessions
        // if both memory and disk ever disagree (cache is authoritative).
        for (path, report) in loaded {
            state.genome_reports.insert(path, report);
        }

        // 3. Walk the workspace for source files honoring gitignore, then
        //    narrow to code files only. `walk_source_files` is the broad
        //    predicate the file watcher uses (covers md/toml/yaml/json for
        //    buffer-reload hooks); here we strip that down to code since
        //    evaluating a README against GoL gives no signal.
        let gitignored = state.gitignored_dirs.clone();
        let files = walk_source_files(&workspace_root, &gitignored);

        // 4. Queue everything missing or stale (mtime newer than the
        //    cached report's timestamp). The launch-time backfill is
        //    visible in LIVE as active (scan-evaluating / scan-queued)
        //    while it runs, but intentionally does NOT populate
        //    `session_touched`: the operator's mental model for LIVE is
        //    "what nit is doing THIS session", and pre-session files
        //    hydrate happens to re-evaluate should not look like
        //    carry-over from a previous run.
        for path in files {
            if !is_code_file(&path) {
                continue;
            }
            if Self::needs_eval(state, &path) {
                self.pending.push_back(path);
            }
        }
        self.total = self.pending.len();
    }

    /// Returns `true` when the cached report for `path` is absent or older
    /// than the file's current mtime.
    fn needs_eval(state: &AppState, path: &Path) -> bool {
        match state.genome_reports.get(path) {
            None => true,
            Some(report) => {
                let mtime = file_mtime_ms(path).unwrap_or(0);
                mtime > report.timestamp_ms
            }
        }
    }

    /// Pump the queue: refill in-flight slots up to the cap, kicking off a
    /// worker thread per newly claimed path. Silent no-op when the cap is
    /// full or the queue is empty.
    pub fn drive(&mut self, worker: &GenomeWorker) {
        while self.dispatched.len() < self.max_in_flight {
            let Some(path) = self.pending.pop_front() else {
                break;
            };
            if !self.dispatched.insert(path.clone()) {
                // Already in flight from an earlier tick — skip without
                // re-dispatching. Counts as done so the progress indicator
                // doesn't deadlock.
                self.done = self.done.saturating_add(1);
                continue;
            }
            if !worker.evaluate_from_disk_workspace_scan(path.clone()) {
                // Thread spawn failed (rare: hit the OS thread ceiling).
                // Drop and count as done so the indicator still finishes.
                self.dispatched.remove(&path);
                self.done = self.done.saturating_add(1);
            }
        }
    }

    /// Called once per workspace-scan result the main loop drains from the
    /// worker. Increments `done` and clears the in-flight slot.
    pub fn note_completed(&mut self, path: &Path) {
        if self.dispatched.remove(path) {
            self.done = self.done.saturating_add(1);
        }
    }

    /// File-watcher event. By design, edits that originate OUTSIDE nit do
    /// not trigger a genome re-evaluation — nit-internal edits (save,
    /// agent FileWrite events) have their own eval paths (`evaluate_save`,
    /// `dispatch_turn_genome_evals`) that keep the cache authoritative.
    /// Routing external edits through the scan queue would pollute FILESCORES
    /// with metrics from changes nit didn't sanction.
    ///
    /// The one case we still act on is file deletion: if the file is gone
    /// on disk, its cached report is a ghost entry and must be purged so
    /// FILESCORES stops showing tiers for files that no longer exist. A
    /// deletion is a structural fact about the workspace, not an edit.
    pub fn note_change(&mut self, state: &mut AppState, path: PathBuf) {
        if !state.settings.genome.genome_context_enabled {
            return;
        }
        if !is_within_workspace_scope(state, &path) {
            return;
        }

        if path.exists() {
            // Existing-file change event: silent no-op. The file's cached
            // report stays as-is until a nit-sanctioned eval path (save or
            // agent turn) replaces it. FILESCORES may show stale metrics
            // until then — that's the intended behaviour: nit only scores
            // code it knows it wrote.
            return;
        }

        // File deleted or moved. Drop the cached report + on-disk JSON.
        let workspace_root = state.workspace_root.clone();
        if state.genome_reports.remove(&path).is_some() {
            nit_core::agent_bus::delete_genome_report(&workspace_root, &path);
        }
        // Also drop from the scan's queues so `total`/`done` stay consistent.
        self.pending.retain(|p| p != &path);
        if self.dispatched.remove(&path) {
            self.done = self.done.saturating_add(1);
        }
    }

    /// True while there are pending or in-flight evals. The UI indicator
    /// shows a progress line while this is `true`.
    pub fn is_scanning(&self) -> bool {
        !self.pending.is_empty() || !self.dispatched.is_empty()
    }

    /// `(done, total)` — files with results landed vs files ever queued.
    /// `total` grows when file-watcher events enqueue more work.
    pub fn progress(&self) -> (usize, usize) {
        (self.done, self.total)
    }

    /// Snapshot of every file currently queued or in-flight. Dispatched
    /// (Evaluating) paths come first so the LIVE tab shows them at the top,
    /// then queued (Queued) paths in FIFO order. The returned list is
    /// session-local — it drains to empty as results land.
    pub fn in_flight_snapshot(&self) -> Vec<(PathBuf, WorkspaceScanItemState)> {
        let mut out: Vec<(PathBuf, WorkspaceScanItemState)> = self
            .dispatched
            .iter()
            .map(|p| (p.clone(), WorkspaceScanItemState::Evaluating))
            .collect();
        // Stable ordering for the dispatched half — HashSet iteration is
        // non-deterministic and reshuffling rows every tick makes the LIVE
        // view hard to read.
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.extend(
            self.pending
                .iter()
                .map(|p| (p.clone(), WorkspaceScanItemState::Queued)),
        );
        out
    }

    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub fn dispatched_count(&self) -> usize {
        self.dispatched.len()
    }

    #[cfg(test)]
    pub fn hydrated(&self) -> bool {
        self.hydrated
    }
}

fn file_mtime_ms(path: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(since_epoch.as_millis() as u64)
}

/// Returns `true` when `path` lives under `workspace_root` and none of its
/// path components are excluded (hidden, gitignored, or in `IGNORED_DIRS`).
/// Paths outside the workspace (e.g. `/tmp`) are rejected so the file
/// watcher doesn't pull in unrelated changes.
fn is_within_workspace_scope(state: &AppState, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(&state.workspace_root) else {
        // Path isn't under the workspace root.
        return false;
    };
    for component in relative.components() {
        let Some(segment) = component.as_os_str().to_str() else {
            continue;
        };
        if is_excluded_directory(segment, &state.gitignored_dirs) {
            return false;
        }
    }
    // `is_excluded_directory` covers gitignored directories and the bare
    // `IGNORED_DIRS` list. Belt-and-braces: re-check IGNORED_DIRS explicitly
    // in case a future caller passes a gitignored_dirs list scrubbed of
    // those entries.
    for component in relative.components() {
        if let Some(segment) = component.as_os_str().to_str() {
            if IGNORED_DIRS.contains(&segment) {
                return false;
            }
        }
    }
    true
}

/// Strip a cached genome report entry by path. Exposed for the file-change
/// path (which wants to remove the state entry AND the on-disk JSON in one
/// call).
#[cfg(test)]
pub fn forget_report(state: &mut AppState, path: &Path) {
    let workspace_root = state.workspace_root.clone();
    if state.genome_reports.remove(path).is_some() {
        nit_core::agent_bus::delete_genome_report(&workspace_root, path);
    }
}

#[cfg(test)]
#[path = "tests/workspace_scan.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/workspace_scan_events.rs"]
mod tests_events;
