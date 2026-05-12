//! Queue and dispatch tests: pending counts, in-flight snapshots, drive
//! cap behaviour. The cap pins the test value rather than relying on
//! `available_parallelism` so assertions are stable across hosts.

use std::time::Duration;

use crate::genome_worker::GenomeWorker;
use crate::workspace_scan::WorkspaceScanRuntime;

use super::{cleanup, drain_until_idle, make_state, temp_workspace, write_file};

#[test]
fn drive_caps_inflight_dispatches() {
    let root = temp_workspace();
    for idx in 0..32 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::with_max_in_flight(3);
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    scan.drive(&worker);
    assert!(scan.dispatched_count() <= 3);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    cleanup(&root);
}

#[test]
fn snapshot_partitions_evaluating_and_queued() {
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
    cleanup(&root);
}

#[test]
fn snapshot_empty_when_idle() {
    let root = temp_workspace();
    let _state = make_state(root.clone());
    let scan = WorkspaceScanRuntime::new();
    assert!(scan.in_flight_snapshot().is_empty());
    cleanup(&root);
}
