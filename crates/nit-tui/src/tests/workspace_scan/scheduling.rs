use std::path::PathBuf;
use std::time::Duration;

use crate::genome_worker::GenomeWorker;
use crate::workspace_scan::WorkspaceScanRuntime;

use super::{cleanup, drain_until_idle, make_state, temp_workspace, write_file};

#[test]
fn scan_processes_many_files_end_to_end() {
    // Integration: many files routed through the real worker. Verifies the
    // in-flight cap doesn't deadlock and every report lands in state.
    let root = temp_workspace();
    let file_count = 16;
    for idx in 0..file_count {
        write_file(
            &root,
            &format!("src/file_{idx:03}.rs"),
            &format!("fn f{idx}() {{ let _ = {idx}; }}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    assert_eq!(scan.pending_count(), file_count);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));

    assert_eq!(state.genome_reports.len(), file_count);
    let (done, total) = scan.progress();
    assert_eq!(done, file_count);
    assert_eq!(total, file_count);
    cleanup(&root);
}

#[test]
fn drive_respects_in_flight_cap() {
    let root = temp_workspace();
    for idx in 0..32 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    // Pin to 3 so the assertion is stable across hosts regardless of
    // available_parallelism.
    let mut scan = WorkspaceScanRuntime::with_max_in_flight(3);
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    scan.drive(&worker);
    assert!(scan.dispatched_count() <= 3);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    cleanup(&root);
}

#[test]
fn workspace_scan_max_in_flight_is_positive_and_bounded() {
    let cap = crate::workspace_scan::workspace_scan_max_in_flight();
    assert!(cap >= 4, "cap must give the scan meaningful parallelism");
    assert!(cap <= 16, "cap must not exceed the hard ceiling");
}

#[test]
fn is_code_file_predicate_covers_expected_languages() {
    use crate::workspace_scan::is_code_file;
    for ext in ["rs", "py", "ts", "tsx", "go", "java", "c", "cpp", "swift"] {
        let p = PathBuf::from(format!("main.{ext}"));
        assert!(is_code_file(&p), "{ext} should be code");
    }
    for ext in ["md", "toml", "yaml", "yml", "json", "txt"] {
        let p = PathBuf::from(format!("x.{ext}"));
        assert!(!is_code_file(&p), "{ext} must be excluded");
    }
}

#[test]
fn in_flight_snapshot_separates_evaluating_from_queued() {
    use crate::workspace_scan::WorkspaceScanItemState;

    let root = temp_workspace();
    for idx in 0..6 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::with_max_in_flight(2);
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    assert_eq!(scan.pending_count(), 6);

    scan.drive(&worker);
    let snapshot = scan.in_flight_snapshot();
    assert_eq!(snapshot.len(), 6);
    let eval_count = snapshot
        .iter()
        .filter(|(_, s)| matches!(s, WorkspaceScanItemState::Evaluating))
        .count();
    let queued_count = snapshot
        .iter()
        .filter(|(_, s)| matches!(s, WorkspaceScanItemState::Queued))
        .count();
    assert_eq!(eval_count, 2);
    assert_eq!(queued_count, 4);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    assert!(scan.in_flight_snapshot().is_empty());
    cleanup(&root);
}

#[test]
fn in_flight_snapshot_is_empty_when_idle() {
    let root = temp_workspace();
    let state = make_state(root.clone());
    let scan = WorkspaceScanRuntime::new();
    let _ = state;
    assert!(scan.in_flight_snapshot().is_empty());
    cleanup(&root);
}

#[test]
fn in_flight_snapshot_orders_evaluating_first() {
    use crate::workspace_scan::WorkspaceScanItemState;

    let root = temp_workspace();
    for idx in 0..4 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::with_max_in_flight(2);
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    scan.drive(&worker);

    // All Evaluating entries must precede all Queued ones so the LIVE view
    // renders active work at the top of the list.
    let snapshot = scan.in_flight_snapshot();
    let mut saw_queued = false;
    for (_, item_state) in &snapshot {
        match item_state {
            WorkspaceScanItemState::Queued => saw_queued = true,
            WorkspaceScanItemState::Evaluating => {
                assert!(
                    !saw_queued,
                    "Evaluating entry appeared after a Queued entry"
                );
            }
        }
    }

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    cleanup(&root);
}
