//! The genome half of the node value function: aggregate per-file
//! [`GenomeReport`]s into a single comparable `(score, tier)`. This answers the
//! "value scalar" open question in `docs/MULTIWAY.md` for the genome dimension;
//! the hard build/test gate half lives in the nit-tui adapter (it needs the
//! gate machinery, which must not leak into this pure crate).

use nit_core::{GenomeReport, GenomeTier};

/// Weight of the tier term in a file's genome score.
pub const W_TIER: f32 = 0.50;
/// Weight of the per-encoder generations-survived term.
pub const W_GENERATIONS: f32 = 0.30;
/// Weight of the cross-encoder consistency term.
pub const W_CONSISTENCY: f32 = 0.20;

/// Generation count that saturates the generations term — the Methuselah
/// ceiling. Surviving longer does not raise the score further.
const GENERATIONS_CEILING: f32 = 2000.0;
/// `GenomeTier` discriminant of the top tier (`Replicator`), normalising the
/// tier term into `[0, 1]`.
const TIER_SPAN: f32 = 4.0;

/// Collapse per-file genome reports into a node-level `(score, tier)`.
///
/// `score ∈ [0, 1]` is the mean of per-file scores — genome quality only; the
/// gate verdict is applied separately by the caller. `tier` is the **minimum**
/// tier across files: the weakest file is the node's bottleneck. An empty slice
/// scores `(0.0, StillLife)`, and the score is coerced away from NaN so callers
/// can order it with [`f32::total_cmp`].
pub fn node_value(reports: &[GenomeReport]) -> (f32, GenomeTier) {
    if reports.is_empty() {
        return (0.0, GenomeTier::StillLife);
    }

    let mut score_sum = 0.0_f32;
    let mut bottleneck = GenomeTier::Replicator;
    for report in reports {
        score_sum += file_score(report);
        bottleneck = bottleneck.min(report.tier);
    }

    let mean = score_sum / reports.len() as f32;
    let score = if mean.is_nan() {
        0.0
    } else {
        mean.clamp(0.0, 1.0)
    };
    (score, bottleneck)
}

fn file_score(report: &GenomeReport) -> f32 {
    let tier = W_TIER * (report.tier as u32 as f32 / TIER_SPAN);
    let generations = W_GENERATIONS * (mean_generations(report) / GENERATIONS_CEILING).min(1.0);
    let consistency = W_CONSISTENCY * report.cross_encoder_consistency;
    tier + generations + consistency
}

fn mean_generations(report: &GenomeReport) -> f32 {
    if report.encoder_scores.is_empty() {
        return 0.0;
    }
    let total: u64 = report
        .encoder_scores
        .iter()
        .map(|score| u64::from(score.generations_survived))
        .sum();
    total as f32 / report.encoder_scores.len() as f32
}
