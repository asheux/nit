use std::path::PathBuf;

use super::sample_report;
use crate::genome_report::GenomeTier;
use crate::genome_report_cache::{count_at_or_above, tier_histogram, GenomeReportMap};

#[test]
fn tier_histogram_counts_per_ladder_position() {
    let mut map = GenomeReportMap::new();
    let tiers = [
        GenomeTier::StillLife,
        GenomeTier::Oscillator,
        GenomeTier::Spaceship,
        GenomeTier::Spaceship,
        GenomeTier::Methuselah,
        GenomeTier::Methuselah,
        GenomeTier::Methuselah,
        GenomeTier::Replicator,
    ];
    for (i, tier) in tiers.iter().enumerate() {
        let path = PathBuf::from(format!("file_{i}.rs"));
        let mut report = sample_report(&path);
        report.tier = *tier;
        map.insert(path, report);
    }

    assert_eq!(tier_histogram(&map), [1, 1, 2, 3, 1]);
    assert_eq!(count_at_or_above(&map, GenomeTier::Spaceship), 6);
    assert_eq!(count_at_or_above(&map, GenomeTier::Replicator), 1);
}
