use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use nit_core::GenomeReport;

use crate::genome_worker::GenomeWorker;
use crate::workspace_scan::WorkspaceScanRuntime;

use super::{cleanup, drain_until_idle, make_state, temp_workspace, write_file};

#[test]
fn external_file_change_does_not_trigger_reeval() {
    // External edits (foreign editor, git pull, terminal) must not
    // invalidate the cache or queue a new eval. Internal writes (save,
    // agent turn) have their own paths; the file watcher is a reload
    // signal only.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");
    let mut report = nit_core::compute_genome_report("fn main() {}\n", &file);
    report.timestamp_ms = 0;
    nit_core::agent_bus::persist_genome_report(&root, &report);

    let mut state = make_state(root.clone());
    state.genome_reports.insert(file.clone(), report);

    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file.clone());

    assert!(state.genome_reports.contains_key(&file));
    assert_eq!(scan.pending_count(), 0);
    assert_eq!(scan.dispatched_count(), 0);
    cleanup(&root);
}

#[test]
fn change_event_outside_workspace_is_ignored() {
    let root = temp_workspace();
    let outside = std::env::temp_dir().join("nit-ws-scan-outside.rs");
    fs::write(&outside, "fn outside() {}\n").unwrap();

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, outside.clone());

    assert_eq!(scan.pending_count(), 0);
    let _ = fs::remove_file(&outside);
    cleanup(&root);
}

#[test]
fn change_event_in_gitignored_dir_is_ignored() {
    let root = temp_workspace();
    let file = write_file(&root, "target/debug/foo.rs", "fn foo() {}\n");

    let mut state = make_state(root.clone());
    state.gitignored_dirs = vec!["target".into()];

    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file.clone());

    assert_eq!(scan.pending_count(), 0);
    cleanup(&root);
}

#[test]
fn delete_event_drops_cached_report_and_disk_file() {
    let root = temp_workspace();
    let file_path = root.join("src/lib.rs");
    let mut report = GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.0,
        tier: nit_core::GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: 0,
        grid_size: 32,
        parsimony: Default::default(),
        function_scores: Vec::new(),
    };
    report.timestamp_ms = 1;
    nit_core::agent_bus::persist_genome_report(&root, &report);

    let mut state = make_state(root.clone());
    state.genome_reports.insert(file_path.clone(), report);

    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file_path.clone());

    assert!(!state.genome_reports.contains_key(&file_path));
    cleanup(&root);
}

#[test]
fn external_change_leaves_cache_unchanged_regardless_of_mtime_skew() {
    // Whether the cached report is newer or older than the file's mtime,
    // an external change event must not touch the cache or enqueue —
    // nit-sanctioned edits (save / agent turn) are the only sanctioned
    // invalidation route during a live session.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    {
        let mut state = make_state(root.clone());
        let report = GenomeReport {
            file_path: file.clone(),
            encoder_scores: Vec::new(),
            cross_encoder_consistency: 0.0,
            tier: nit_core::GenomeTier::StillLife,
            recommendations: Vec::new(),
            timestamp_ms: u64::MAX,
            grid_size: 32,
            parsimony: Default::default(),
            function_scores: Vec::new(),
        };
        state.genome_reports.insert(file.clone(), report);

        let mut scan = WorkspaceScanRuntime::new();
        scan.note_change(&mut state, file.clone());

        assert_eq!(scan.pending_count(), 0);
        assert!(state.genome_reports.contains_key(&file));
    }

    {
        let mut state = make_state(root.clone());
        let report = GenomeReport {
            file_path: file.clone(),
            encoder_scores: Vec::new(),
            cross_encoder_consistency: 0.0,
            tier: nit_core::GenomeTier::StillLife,
            recommendations: Vec::new(),
            timestamp_ms: 0,
            grid_size: 32,
            parsimony: Default::default(),
            function_scores: Vec::new(),
        };
        state.genome_reports.insert(file.clone(), report);

        let mut scan = WorkspaceScanRuntime::new();
        scan.note_change(&mut state, file.clone());

        assert_eq!(scan.pending_count(), 0);
        assert!(state.genome_reports.contains_key(&file));
    }

    cleanup(&root);
}

#[test]
fn note_change_ignores_non_code_extensions() {
    let root = temp_workspace();
    let md = write_file(&root, "README.md", "# doc\n");
    let json = write_file(&root, "pkg.json", "{}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, md);
    scan.note_change(&mut state, json);

    assert_eq!(scan.pending_count(), 0);
    cleanup(&root);
}

#[test]
fn note_change_with_unknown_extension_is_noop() {
    let root = temp_workspace();
    let file = write_file(&root, "assets/data.bin", "\0");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.note_change(&mut state, file);

    assert_eq!(scan.pending_count(), 0);
}

#[test]
fn repeated_note_change_never_queues_existing_files() {
    // The file-watcher emits dozens of events for the same path during an
    // active edit burst; none should queue the file.
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

#[test]
fn change_event_path_not_under_workspace_returns_early() {
    let root = temp_workspace();
    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();

    let outside = PathBuf::from("/definitely/not/in/the/workspace/lib.rs");
    scan.note_change(&mut state, outside);

    assert_eq!(scan.pending_count(), 0);
    cleanup(&root);
}

#[test]
fn external_edit_during_in_flight_eval_is_silent_no_op() {
    // External edits that land while an internal eval is running on the
    // same path must not schedule a follow-up. The in-flight eval finishes
    // normally; the queue stays empty afterwards.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::with_max_in_flight(1);
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    scan.drive(&worker);
    assert_eq!(scan.dispatched_count(), 1);

    scan.note_change(&mut state, file.clone());
    assert_eq!(scan.pending_count(), 0);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    assert!(!scan.is_scanning());
    assert_eq!(scan.pending_count(), 0);
    cleanup(&root);
}

#[test]
fn external_deletion_purges_cache_and_queue() {
    // A file that no longer exists on disk can't have a meaningful cached
    // report; the delete must purge state + cache so FILESCORES doesn't
    // display a tier for a path that's gone.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let mut state = make_state(root.clone());
    let report = nit_core::compute_genome_report("fn main() {}\n", &file);
    state.genome_reports.insert(file.clone(), report);

    let mut scan = WorkspaceScanRuntime::with_max_in_flight(1);
    fs::remove_file(&file).unwrap();
    scan.note_change(&mut state, file.clone());

    assert!(!state.genome_reports.contains_key(&file));
    assert_eq!(scan.pending_count(), 0);
    assert_eq!(scan.dispatched_count(), 0);
    cleanup(&root);
}
