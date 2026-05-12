//! Plain-text formatters for `GenomeReport` and `GenomeDiff`. Used by the
//! CLI / log lines / agent feedback prompt. Each function appends lines to
//! the returned `String`; callers concatenate as needed.

use super::{
    EncoderDiff, EncoderScore, GenomeDiff, GenomeReport, ParsimonyInfo, RecommendationSeverity,
};

pub fn format_genome_report(report: &GenomeReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("[genome report] {}\n", report.file_path.display()));
    out.push_str(&format!(
        "Quality: {} (tier {}, consistency {:.2})\n",
        report.quality_level(),
        report.tier.numeral(),
        report.cross_encoder_consistency
    ));
    out.push_str(&format!(
        "Tier: {} ({}) [grid {}x{}]\n",
        report.tier.numeral(),
        report.tier.name(),
        report.grid_size,
        report.grid_size,
    ));
    out.push_str(&format!(
        "Cross-encoder consistency: {:.2}\n",
        report.cross_encoder_consistency
    ));
    if let Some(line) = format_parsimony_line(&report.parsimony) {
        out.push_str(&line);
    }
    out.push('\n');

    out.push_str("Encoder scores:\n");
    for score in &report.encoder_scores {
        out.push_str(&format_encoder_block(score));
    }

    if !report.recommendations.is_empty() {
        out.push_str("\nRecommendations:\n");
        for rec in &report.recommendations {
            out.push_str(&format!(
                "  [{}] {}\n",
                severity_label(rec.severity),
                rec.message
            ));
        }
    }
    out
}

pub fn format_genome_diff(diff: &GenomeDiff) -> String {
    let mut out = String::new();
    out.push_str(&format!("[genome diff] {}\n", diff.file_path.display()));

    let tier_arrow = match diff.tier_after.cmp(&diff.tier_before) {
        std::cmp::Ordering::Greater => "upgraded",
        std::cmp::Ordering::Less => "regressed",
        std::cmp::Ordering::Equal => "unchanged",
    };
    out.push_str(&format!(
        "Tier: {} -> {} ({})\n",
        diff.tier_before.numeral(),
        diff.tier_after.numeral(),
        tier_arrow,
    ));

    let consistency_delta = diff.consistency_after - diff.consistency_before;
    out.push_str(&format!(
        "Consistency: {:.2} -> {:.2} ({:+.2})\n\n",
        diff.consistency_before, diff.consistency_after, consistency_delta,
    ));

    out.push_str(&format!(
        "{:<20} {:>10} {:>10} {:>10}\n",
        "Encoder", "Density", "Components", "Generations"
    ));
    for ed in &diff.encoder_diffs {
        out.push_str(&format_delta_line(ed));
    }
    out
}

fn format_parsimony_line(p: &ParsimonyInfo) -> Option<String> {
    if p.fn_count == 0 && p.comment_ratio == 0.0 {
        return None;
    }
    let bloat_tag = if p.bloat_detected {
        " [BLOAT — tier capped]"
    } else {
        ""
    };
    Some(format!(
        "Parsimony: {} fns, avg {:.1} lines/fn, {:.0}% tiny, {:.0}% comments{}\n",
        p.fn_count,
        p.avg_fn_body_lines,
        p.tiny_fn_fraction * 100.0,
        p.comment_ratio * 100.0,
        bloat_tag,
    ))
}

fn format_encoder_block(score: &EncoderScore) -> String {
    let cycle = match score.cycle_period {
        Some(p) => format!(", cycle={p}"),
        None => String::new(),
    };
    format!(
        "  {}: density={:.2}, components={}, generations={}, peak_pop={}, growth={}{}\n",
        score.encoder.label(),
        score.density,
        score.components,
        score.generations_survived,
        score.peak_population,
        score.growth_class.label(),
        cycle,
    )
}

fn format_delta_line(ed: &EncoderDiff) -> String {
    format!(
        "{:<20} {:>+10.2} {:>+10} {:>+10}\n",
        ed.encoder.label(),
        ed.density_delta,
        ed.components_delta,
        ed.generations_delta,
    )
}

fn severity_label(severity: RecommendationSeverity) -> &'static str {
    match severity {
        RecommendationSeverity::Critical => "CRITICAL",
        RecommendationSeverity::Warning => "WARNING",
        RecommendationSeverity::Info => "INFO",
    }
}
