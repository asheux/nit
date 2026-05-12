use super::dedupe::{demote_findings_inside_critical_fns, parse_line_range};
use crate::genome_report::{GenomeRecommendation, RecommendationSeverity};

fn rec(metric: &str, severity: RecommendationSeverity, location: &str) -> GenomeRecommendation {
    GenomeRecommendation {
        metric: metric.into(),
        severity,
        message: String::new(),
        location: Some(location.into()),
    }
}

#[test]
fn parse_line_range_handles_both_formats() {
    assert_eq!(parse_line_range("140-1114"), Some((140, 1114)));
    assert_eq!(parse_line_range("run_loop:140-1114"), Some((140, 1114)));
    assert_eq!(parse_line_range("garbage"), None);
    assert_eq!(parse_line_range("9999-1"), None); // end < start
}

#[test]
fn nesting_warnings_inside_critical_fn_get_demoted_to_info() {
    let mut recs = vec![
        rec(
            "cyclomatic_complexity",
            RecommendationSeverity::Critical,
            "run_loop:140-1114",
        ),
        rec("nesting_depth", RecommendationSeverity::Warning, "270-290"),
        rec("nesting_depth", RecommendationSeverity::Warning, "972-1103"),
        // Outside the critical range — must NOT be demoted.
        rec(
            "nesting_depth",
            RecommendationSeverity::Warning,
            "1500-1520",
        ),
        // identifier_uniqueness within the critical fn — also demoted.
        rec(
            "identifier_uniqueness",
            RecommendationSeverity::Warning,
            "run_loop:140-1114",
        ),
    ];
    demote_findings_inside_critical_fns(&mut recs);

    assert_eq!(recs[0].severity, RecommendationSeverity::Critical); // unchanged
    assert_eq!(recs[1].severity, RecommendationSeverity::Info); // inside
    assert_eq!(recs[2].severity, RecommendationSeverity::Info); // inside
    assert_eq!(recs[3].severity, RecommendationSeverity::Warning); // outside, kept
    assert_eq!(recs[4].severity, RecommendationSeverity::Info); // inside
}

#[test]
fn dedupe_is_noop_when_no_critical_present() {
    let mut recs = vec![rec(
        "nesting_depth",
        RecommendationSeverity::Warning,
        "270-290",
    )];
    demote_findings_inside_critical_fns(&mut recs);
    assert_eq!(recs[0].severity, RecommendationSeverity::Warning);
}

#[test]
fn token_entropy_inside_critical_fn_gets_demoted() {
    // Low token diversity in a sub-range inside the critical fn is
    // monotonous code that the critical's owning fn refactor will address.
    let mut recs = vec![
        rec(
            "cyclomatic_complexity",
            RecommendationSeverity::Critical,
            "run_loop:140-1114",
        ),
        rec("token_entropy", RecommendationSeverity::Warning, "200-300"),
        rec(
            "token_entropy",
            RecommendationSeverity::Warning,
            "1500-1600",
        ),
    ];
    demote_findings_inside_critical_fns(&mut recs);
    assert_eq!(recs[1].severity, RecommendationSeverity::Info); // inside
    assert_eq!(recs[2].severity, RecommendationSeverity::Warning); // outside, kept
}

#[test]
fn cognitive_complexity_duplicate_inside_critical_is_demoted() {
    // cognitive_complexity for the same function the cyclomatic critical
    // already flagged is structurally redundant — both point at the same
    // fix. Operators should see one item, not two.
    let mut recs = vec![
        rec(
            "cyclomatic_complexity",
            RecommendationSeverity::Critical,
            "run_loop:140-1114",
        ),
        rec(
            "cognitive_complexity",
            RecommendationSeverity::Warning,
            "140-1114",
        ),
    ];
    demote_findings_inside_critical_fns(&mut recs);
    assert_eq!(recs[1].severity, RecommendationSeverity::Info);
}

#[test]
fn unrelated_metrics_are_not_demoted() {
    // density / components reflect file-level issues not specific to any
    // one function. Even when their nominal location overlaps a critical
    // fn, they stay surfaced — fixing the giant fn won't fix density.
    let mut recs = vec![
        rec(
            "cyclomatic_complexity",
            RecommendationSeverity::Critical,
            "run_loop:140-1114",
        ),
        rec("density", RecommendationSeverity::Warning, "200-300"),
        rec("components", RecommendationSeverity::Warning, "500-600"),
    ];
    demote_findings_inside_critical_fns(&mut recs);
    assert_eq!(recs[1].severity, RecommendationSeverity::Warning);
    assert_eq!(recs[2].severity, RecommendationSeverity::Warning);
}
