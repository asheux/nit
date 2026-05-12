//! Worker-result handling: the workspace_scan flag must be set on every
//! returned result, backfill drains LIVE on completion, and a fresh
//! runtime constructed after a prior session starts empty.

use std::time::{Duration, Instant};

use crate::genome_worker::GenomeWorker;
use crate::workspace_scan::WorkspaceScanRuntime;

use super::{cleanup, drain_until_idle, make_state, temp_workspace, write_file};

#[test]
fn worker_result_tags_workspace_scan_true() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let worker = GenomeWorker::new();
    assert!(worker.evaluate_from_disk_workspace_scan(file.clone()));

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(result) = worker.rx.try_recv() {
            assert!(result.workspace_scan);
            assert!(!result.shadow);
            assert!(!result.save_eval);
            assert_eq!(result.path, file);
            assert!(result.report.is_some());
            break;
        }
        if Instant::now() >= deadline {
            panic!("worker did not produce a result within 10s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cleanup(&root);
}

#[test]
fn backfill_drains_to_empty_on_completion() {
    let root = temp_workspace();
    for idx in 0..4 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    assert_eq!(scan.pending_count() + scan.dispatched_count(), 4);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    assert!(scan.in_flight_snapshot().is_empty());
    cleanup(&root);
}

#[test]
fn fresh_runtime_after_prior_session_starts_empty() {
    let root = temp_workspace();
    for idx in 0..3 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let worker = GenomeWorker::new();

    {
        let mut scan = WorkspaceScanRuntime::new();
        scan.hydrate(&mut state);
        drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    }

    let scan_b = WorkspaceScanRuntime::new();
    assert_eq!(scan_b.pending_count(), 0);
    assert_eq!(scan_b.dispatched_count(), 0);
    let (done_b, total_b) = scan_b.progress();
    assert_eq!(done_b, 0);
    assert_eq!(total_b, 0);
    cleanup(&root);
}
