//! Workspace-wide genome scan driver tests. Sub-modules cover hydration,
//! external change events, queue scheduling, cache lifecycle, ignore
//! filtering, gc, and runner-completion flows. Helpers below are shared by
//! every sub-module so each can scaffold a temp workspace in one line.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nit_core::{AppState, Buffer};

use crate::genome_worker::GenomeWorker;
use crate::workspace_scan::WorkspaceScanRuntime;

#[path = "workspace_scan/cache_lifecycle.rs"]
mod cache_lifecycle;
#[path = "workspace_scan/change_events.rs"]
mod change_events;
#[path = "workspace_scan/filters.rs"]
mod filters;
#[path = "workspace_scan/gc.rs"]
mod gc;
#[path = "workspace_scan/hydrate.rs"]
mod hydrate;
#[path = "workspace_scan/queue.rs"]
mod queue;
#[path = "workspace_scan/runner_completions.rs"]
mod runner_completions;
#[path = "workspace_scan/scheduling.rs"]
mod scheduling;

pub(super) fn temp_workspace() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // (pid, ts, counter) — counter dominates because macOS's SystemTime is
    // coarser than nanosecond; concurrent `now()` calls in the same
    // microsecond would otherwise produce identical directory names.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nit-ws-scan-{pid}-{ts}-{n}"));
    fs::create_dir_all(&dir).unwrap();
    // Canonicalise so strip_prefix against workspace_root doesn't trip on
    // macOS's `/var` → `/private/var` symlink.
    fs::canonicalize(&dir).unwrap_or(dir)
}

pub(super) fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

pub(super) fn make_state(root: PathBuf) -> AppState {
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
    state.settings.genome.genome_context_enabled = true;
    state
}

pub(super) fn write_file(root: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

pub(super) fn drain_until_idle(
    state: &mut AppState,
    scan: &mut WorkspaceScanRuntime,
    worker: &GenomeWorker,
    max_wait: Duration,
) {
    let start = Instant::now();
    loop {
        scan.drive(worker);
        while let Ok(result) = worker.rx.try_recv() {
            assert!(result.workspace_scan, "non-scan result leaked");
            if let Some(report) = result.report {
                nit_core::agent_bus::persist_genome_report(&state.workspace_root, &report);
                state.genome_reports.insert(result.path.clone(), report);
            }
            scan.note_completed(&result.path);
        }
        if !scan.is_scanning() {
            return;
        }
        if start.elapsed() > max_wait {
            panic!("scan did not drain within {max_wait:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
