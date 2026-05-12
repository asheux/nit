//! Standalone change-event tests for the workspace_scan driver. Lives at
//! the tests/ root (not under tests/workspace_scan/) so it can be invoked
//! by name even when the parent `mod workspace_scan;` is not in scope —
//! useful when bisecting an event-flow regression.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, path::Path};

use nit_core::{AppState, Buffer};

use crate::workspace_scan::WorkspaceScanRuntime;

fn temp_workspace() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nit-ws-events-{pid}-{ts}-{n}"));
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap_or(dir)
}

fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

fn make_state(root: PathBuf) -> AppState {
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
    state.settings.genome.genome_context_enabled = true;
    state
}

fn write_file(root: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn change_event_outside_workspace_ignored() {
    let root = temp_workspace();
    let outside = std::env::temp_dir().join("nit-ws-events-outside.rs");
    fs::write(&outside, "fn outside() {}\n").unwrap();

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, outside.clone());

    assert_eq!(scan.pending_count(), 0);
    let _ = fs::remove_file(&outside);
    cleanup(&root);
}

#[test]
fn repeated_change_events_do_not_enqueue() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();

    for _ in 0..5 {
        scan.note_change(&mut state, file.clone());
    }

    assert_eq!(scan.pending_count(), 0);
    assert_eq!(scan.dispatched_count(), 0);
    cleanup(&root);
}
