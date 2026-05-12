//! Flat sibling tests for the genome cache GC pathway. Mirrors a subset
//! of `tests/genome_storage/gc.rs` so the GC routines can be invoked by
//! name without entering the `genome_storage` parent module — useful when
//! bisecting a regression that only fires in isolation.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::genome_report::{GenomeReport, GenomeTier, ParsimonyInfo};
use crate::genome_storage::cache::{load_genome_reports, persist_genome_report};
use crate::genome_storage::migrations::gc_genome_cache;
use crate::genome_storage::schema::{report_path, MAX_CACHE_AGE_SECS};
use crate::test_helpers::temp_dir;

fn forge_report(file_path: &Path) -> GenomeReport {
    GenomeReport {
        file_path: file_path.to_path_buf(),
        encoder_scores: Vec::new(),
        cross_encoder_consistency: 0.5,
        tier: GenomeTier::Spaceship,
        recommendations: Vec::new(),
        timestamp_ms: 1_700_000_000_000,
        grid_size: 32,
        parsimony: ParsimonyInfo::default(),
        function_scores: Vec::new(),
    }
}

fn set_mtime(path: &Path, time: SystemTime) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for set_modified");
    file.set_modified(time).expect("set_modified");
}

#[test]
fn gc_evicts_aged_reports_from_flat_test() {
    let ws = temp_dir("genome-gc-flat-age");
    let source = ws.join("crates/example/src/aged.rs");
    persist_genome_report(&ws, &forge_report(&source));

    let file = report_path(&ws, &source);
    let stale = SystemTime::now() - Duration::from_secs(MAX_CACHE_AGE_SECS + 60);
    set_mtime(&file, stale);

    gc_genome_cache(&ws);
    assert!(load_genome_reports(&ws).is_empty());
}

#[test]
fn gc_keeps_fresh_reports_from_flat_test() {
    let ws = temp_dir("genome-gc-flat-fresh");
    let source = ws.join("crates/example/src/fresh.rs");
    persist_genome_report(&ws, &forge_report(&source));

    gc_genome_cache(&ws);
    assert!(load_genome_reports(&ws).contains_key(&source));
}
