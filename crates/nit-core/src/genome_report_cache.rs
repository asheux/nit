use std::collections::HashMap;
use std::path::PathBuf;

use crate::genome_report::{GenomeReport, GenomeTier};

pub type GenomeReportMap = HashMap<PathBuf, GenomeReport>;

pub fn tier_histogram(map: &GenomeReportMap) -> [u32; 5] {
    let mut hist = [0u32; 5];
    for report in map.values() {
        hist[report.tier as usize] += 1;
    }
    hist
}

pub fn count_at_or_above(map: &GenomeReportMap, min_tier: GenomeTier) -> usize {
    let floor = min_tier as usize;
    let hist = tier_histogram(map);
    hist[floor..].iter().map(|n| *n as usize).sum()
}
