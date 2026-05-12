//! Tests for the shared test helpers themselves. Each helper must be
//! callable from a sub-module, and `state_for_test*` must construct an
//! `AppState` that subsequent tests can mutate.

use super::*;

#[test]
fn state_for_test_constructs_minimal_app_state() {
    let state = state_for_test();
    assert!(state.workspace_root.as_os_str() == ".");
    assert!(!state.buffers.is_empty());
}

#[test]
fn state_for_test_in_workspace_creates_temp_dir() {
    let state = state_for_test_in_workspace("helpers-smoke");
    assert!(state.workspace_root.exists());
    assert!(state.workspace_root.is_dir());
    let _ = std::fs::remove_dir_all(&state.workspace_root);
}

#[test]
fn seeded_genome_report_sets_requested_tier() {
    let path = std::path::PathBuf::from("a.rs");
    let report = seeded_genome_report(path.clone(), nit_core::GenomeTier::Methuselah);
    assert_eq!(report.file_path, path);
    assert_eq!(report.tier, nit_core::GenomeTier::Methuselah);
    assert_eq!(report.cross_encoder_consistency, 0.42);
}

#[test]
fn seeded_genome_report_default_grid_size() {
    let report = seeded_genome_report(
        std::path::PathBuf::from("b.rs"),
        nit_core::GenomeTier::Spaceship,
    );
    assert_eq!(report.grid_size, 32);
    assert!(report.encoder_scores.is_empty());
}
