use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use nit_core::AppState;

use crate::file_watcher::{walk_source_files, IGNORED_DIRS};
use crate::genome_worker::GenomeWorker;

use super::scope::{is_code_file, workspace_scan_max_in_flight, WorkspaceScanItemState};
use super::util::{file_mtime_ms, is_within_workspace_scope};

/// Driver for the background genome scan. One instance lives in `run_loop`
/// alongside `GenomeWorker`; the main loop calls `hydrate` once at startup,
/// `drive` every tick, and `note_completed` when a workspace-scan result
/// lands on the worker's receiver.
pub struct WorkspaceScanRuntime {
    pending: VecDeque<PathBuf>,
    // Dedup guard against re-dispatching a path whose worker is still alive.
    dispatched: HashSet<PathBuf>,
    // Cumulative queued count; denominator in the UI progress line.
    total: usize,
    // Cumulative finished count; resets on a fresh seed (operator rescan).
    done: usize,
    // Idempotent guard so repeated `load_cache` calls are no-ops within a
    // session.
    cache_loaded: bool,
    // Idempotent guard so the workspace walk runs at most once per session
    // unless `rescan` clears it.
    walked: bool,
    // Computed once at construction so tests can override via
    // `with_max_in_flight` without touching globals.
    max_in_flight: usize,
}

impl Default for WorkspaceScanRuntime {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            dispatched: HashSet::new(),
            total: 0,
            done: 0,
            cache_loaded: false,
            walked: false,
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

    /// Launch-time cache load. Reads `.nit/genome/` into
    /// `state.genome_reports`, drops entries whose files no longer exist
    /// (including their on-disk JSON), and sweeps the legacy cache layout.
    /// Does NOT walk the workspace — that's deferred to [`Self::hydrate`].
    ///
    /// Idempotent + silent no-op when `genome_context_enabled` is off.
    pub fn load_cache(&mut self, state: &mut AppState) {
        if self.cache_loaded {
            return;
        }
        self.cache_loaded = true;

        if !state.settings.genome.genome_context_enabled {
            return;
        }

        let workspace_root = state.workspace_root.clone();
        let mut loaded = nit_core::agent_bus::load_genome_reports(&workspace_root);

        let stale: Vec<PathBuf> = loaded
            .keys()
            .filter(|path| !path.exists())
            .cloned()
            .collect();
        for path in stale {
            loaded.remove(&path);
            nit_core::agent_bus::delete_genome_report(&workspace_root, &path);
        }

        nit_core::agent_bus::gc_genome_cache(&workspace_root);

        // The in-memory map is cleared at startup so `insert` won't conflict.
        // If memory and disk ever disagree on a follow-up session, the disk
        // cache is authoritative.
        for (path, report) in loaded {
            state.genome_reports.insert(path, report);
        }
    }

    /// Full hydrate: ensure the disk cache is loaded, walk the workspace,
    /// queue every code file whose cached report is missing or stale.
    /// Idempotent — runs at most once per session unless [`Self::rescan`]
    /// clears the guard. Silent no-op when `genome_context_enabled` is off.
    pub fn hydrate(&mut self, state: &mut AppState) {
        if self.walked {
            return;
        }
        self.walked = true;

        if !state.settings.genome.genome_context_enabled {
            return;
        }

        self.load_cache(state);

        let workspace_root = state.workspace_root.clone();
        let gitignored = state.gitignored_dirs.clone();
        // `walk_source_files` is the broad predicate the file watcher uses
        // (covers md/toml/yaml/json for buffer-reload hooks); narrow to code
        // here since evaluating a README against GoL produces no signal.
        let files = walk_source_files(&workspace_root, &gitignored);

        // The walk-time backfill is visible in LIVE as scan-evaluating /
        // scan-queued while it runs, but intentionally does NOT populate
        // `session_touched`: the operator's mental model for LIVE is "what
        // nit is doing THIS session", and pre-session files the rescan
        // happens to re-evaluate should not look like carry-over.
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

    /// Operator-triggered rescan: reset the walked guard, clear progress,
    /// re-run [`Self::hydrate`]. No-op while a previous scan is still in
    /// flight — clicking again during a scan shouldn't re-queue files
    /// (`drive`'s dedup guard would discard them, but `total` would over-
    /// count).
    pub fn rescan(&mut self, state: &mut AppState) {
        if self.is_scanning() {
            return;
        }
        self.walked = false;
        self.done = 0;
        self.total = 0;
        self.hydrate(state);
    }

    /// Dry walk: count total code files and how many need a fresh eval.
    /// Does NOT queue evaluations — runs at startup so the EVAL button can
    /// show "stale/total" before the operator clicks.
    pub fn count_workspace(state: &AppState) -> (usize, usize) {
        if !state.settings.genome.genome_context_enabled {
            return (0, 0);
        }
        let files = walk_source_files(&state.workspace_root, &state.gitignored_dirs);
        let mut total = 0usize;
        let mut stale = 0usize;
        for path in files {
            if !is_code_file(&path) {
                continue;
            }
            total += 1;
            if Self::needs_eval(state, &path) {
                stale += 1;
            }
        }
        (stale, total)
    }

    fn needs_eval(state: &AppState, path: &Path) -> bool {
        match state.genome_reports.get(path) {
            None => true,
            Some(report) => {
                let mtime = file_mtime_ms(path).unwrap_or(0);
                mtime > report.timestamp_ms
            }
        }
    }

    /// Refill in-flight slots up to the cap, kicking off a worker thread
    /// per newly claimed path. Silent no-op when the cap is full or the
    /// queue is empty.
    pub fn drive(&mut self, worker: &GenomeWorker) {
        while self.dispatched.len() < self.max_in_flight {
            let Some(path) = self.pending.pop_front() else {
                break;
            };
            if !self.dispatched.insert(path.clone()) {
                // Already in flight from an earlier tick. Count as done so
                // the progress indicator doesn't deadlock.
                self.done = self.done.saturating_add(1);
                continue;
            }
            if !worker.evaluate_from_disk_workspace_scan(path.clone()) {
                // Rare: hit the OS thread ceiling. Drop and count as done.
                self.dispatched.remove(&path);
                self.done = self.done.saturating_add(1);
            }
        }
    }

    /// Called per workspace-scan result the main loop drains from the
    /// worker. Increments `done` and clears the in-flight slot.
    pub fn note_completed(&mut self, path: &Path) {
        if self.dispatched.remove(path) {
            self.done = self.done.saturating_add(1);
        }
    }

    /// File-watcher event. By design, edits originating OUTSIDE nit do NOT
    /// trigger a re-eval — nit-internal edits (save, agent FileWrite) own
    /// their own eval paths (`evaluate_save`, `dispatch_turn_genome_evals`)
    /// that keep the cache authoritative. Routing external edits through
    /// the scan queue would pollute FILESCORES with metrics from changes
    /// nit didn't sanction.
    ///
    /// We still act on deletions: if the file is gone on disk, its cached
    /// report is a ghost entry and must be purged so FILESCORES stops
    /// showing tiers for files that no longer exist.
    pub fn note_change(&mut self, state: &mut AppState, path: PathBuf) {
        if !state.settings.genome.genome_context_enabled {
            return;
        }
        if !is_within_workspace_scope(state, &path, IGNORED_DIRS) {
            return;
        }

        if path.exists() {
            return;
        }

        let workspace_root = state.workspace_root.clone();
        if state.genome_reports.remove(&path).is_some() {
            nit_core::agent_bus::delete_genome_report(&workspace_root, &path);
        }
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
    pub fn progress(&self) -> (usize, usize) {
        (self.done, self.total)
    }

    /// Snapshot of every file currently queued or in-flight. Dispatched
    /// (Evaluating) paths come first so the LIVE tab shows them at the
    /// top; queued paths follow in FIFO order. Drains to empty as results
    /// land.
    pub fn in_flight_snapshot(&self) -> Vec<(PathBuf, WorkspaceScanItemState)> {
        let mut out: Vec<(PathBuf, WorkspaceScanItemState)> = self
            .dispatched
            .iter()
            .map(|p| (p.clone(), WorkspaceScanItemState::Evaluating))
            .collect();
        // HashSet iteration is non-deterministic — sort so LIVE doesn't
        // reshuffle rows every tick.
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
        self.walked
    }
}
