//! Dedup pass for `generate_recommendations`. When a function is flagged
//! `cyclomatic_complexity` Critical, sub-findings inside its line range
//! are symptoms of the same problem; demoting them to Info collapses the
//! report onto the actionable root cause.

use crate::genome_report::{GenomeRecommendation, RecommendationSeverity};

/// Metric names whose Warning-level findings inside a critical function's
/// line range are demoted to Info. `cognitive_complexity` is special-cased
/// at every severity below since it's a structural duplicate of the
/// cyclomatic critical for the same span.
const SYMPTOMATIC: &[&str] = &[
    "nesting_depth",
    "identifier_uniqueness",
    "token_entropy",
    "cognitive_complexity",
];

pub(crate) fn demote_findings_inside_critical_fns(recs: &mut [GenomeRecommendation]) {
    let critical_ranges: Vec<(u32, u32)> = recs
        .iter()
        .filter(|r| {
            r.metric == "cyclomatic_complexity" && r.severity == RecommendationSeverity::Critical
        })
        .filter_map(|r| r.location.as_deref().and_then(parse_line_range))
        .collect();
    if critical_ranges.is_empty() {
        return;
    }
    for rec in recs.iter_mut() {
        // Other metrics demote only from Warning so Critical entries don't
        // get flipped down; `cognitive_complexity` is the one carve-out
        // since it's a structural duplicate of the cyclomatic critical.
        if rec.severity == RecommendationSeverity::Critical && rec.metric != "cognitive_complexity"
        {
            continue;
        }
        if !SYMPTOMATIC.contains(&rec.metric.as_str()) {
            continue;
        }
        let Some((rs, re)) = rec.location.as_deref().and_then(parse_line_range) else {
            continue;
        };
        if critical_ranges.iter().any(|&(cs, ce)| rs >= cs && re <= ce) {
            rec.severity = RecommendationSeverity::Info;
        }
    }
}

/// Parse the trailing `START-END` portion of a recommendation `location`.
/// Accepts both `"run_loop:140-1114"` (function recs) and `"140-1114"`
/// (nesting recs). Returns `None` on malformed input — caller treats that
/// as "can't dedupe" (safe; leaves the rec alone).
pub(crate) fn parse_line_range(location: &str) -> Option<(u32, u32)> {
    let tail = location.rsplit(':').next()?;
    let (start, end) = tail.split_once('-')?;
    let start = start.trim().parse::<u32>().ok()?;
    let end = end.trim().parse::<u32>().ok()?;
    if end < start {
        return None;
    }
    Some((start, end))
}
