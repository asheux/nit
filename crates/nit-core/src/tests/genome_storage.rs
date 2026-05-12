//! On-disk genome cache tests. Sub-modules cover round-trip persistence,
//! garbage collection, and tier histograms. Shared fixtures live here so
//! each sub-module can construct a `GenomeReport` without duplicating the
//! field set.

use std::path::Path;

use crate::genome_report::{GenomeReport, GenomeTier, ParsimonyInfo};

#[path = "genome_storage/gc.rs"]
mod gc;
#[path = "genome_storage/histograms.rs"]
mod histograms;
#[path = "genome_storage/round_trip.rs"]
mod round_trip;

pub(super) fn sample_report(file_path: &Path) -> GenomeReport {
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

#[allow(dead_code)]
pub(super) fn sample_report_with_tier(file_path: &Path, tier: GenomeTier) -> GenomeReport {
    let mut report = sample_report(file_path);
    report.tier = tier;
    report
}
