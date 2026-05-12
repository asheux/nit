//! Cache-entry purge tests: deleted files, external deletions, phantom
//! reports left after a file disappears from disk.

use std::fs;

use nit_core::GenomeReport;

use crate::workspace_scan::WorkspaceScanRuntime;

use super::{cleanup, make_state, temp_workspace, write_file};

#[test]
fn purges_phantom_report_on_hydrate() {
    let root = temp_workspace();
    let deleted_path = root.join("src").join("gone.rs");
    let phantom = nit_core::compute_genome_report("fn gone() {}\n", &deleted_path);
    nit_core::agent_bus::persist_genome_report(&root, &phantom);

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    scan.hydrate(&mut state);

    assert!(
        !state.genome_reports.contains_key(&deleted_path),
        "deleted file report should be purged"
    );
    cleanup(&root);
}

#[test]
fn delete_event_clears_cached_report() {
    let root = temp_workspace();
    let file_path = root.join("src/lib.rs");
    let mut report = GenomeReport {
        file_path: file_path.clone(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.0,
        tier: nit_core::GenomeTier::StillLife,
        recommendations: Vec::new(),
        timestamp_ms: 1,
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
fn external_deletion_drops_cache_and_queue() {
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
    cleanup(&root);
}
