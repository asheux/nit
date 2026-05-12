//! Parsimony-detector tests. Exercise comment-padding, tiny-fn fraction,
//! and duplicate-comment detection at the `compute_genome_report_fast`
//! boundary.

use std::path::Path;

use crate::genome_report::{compute_genome_report_fast, GenomeTier};

#[test]
fn natural_code_does_not_trip_bloat() {
    let code = r#"
fn aggregate(items: &[i32]) -> i32 {
    let mut sum = 0;
    for v in items {
        sum += v;
    }
    sum
}

fn average(items: &[i32]) -> i32 {
    if items.is_empty() {
        return 0;
    }
    aggregate(items) / items.len() as i32
}

fn pipeline(items: &[i32]) -> i32 {
    let summed = aggregate(items);
    let avg = average(items);
    summed + avg
}
"#;
    let report = compute_genome_report_fast(code, Path::new("clean.rs"));
    assert!(
        !report.parsimony.bloat_detected,
        "well-shaped code should not be flagged"
    );
}

#[test]
fn duplicate_consecutive_comments_caps_tier() {
    // Two identical adjacent `///` lines is always a merge accident — the
    // parsimony detector caps the file at Methuselah.
    let mut body = String::new();
    body.push_str("/// Header doc.\n");
    body.push_str("/// Header doc.\n");
    body.push_str("fn f() {}\n");
    for i in 0..40 {
        body.push_str(&format!("fn g{i}() {{ let _ = {i}; }}\n"));
    }
    let report = compute_genome_report_fast(&body, Path::new("dup.rs"));
    assert!(report.parsimony.duplicate_comment_lines >= 1);
    assert!(matches!(
        report.tier,
        GenomeTier::Methuselah
            | GenomeTier::Spaceship
            | GenomeTier::StillLife
            | GenomeTier::Oscillator
    ));
}

#[test]
fn comment_heavy_file_records_high_comment_ratio() {
    let mut body = String::new();
    for i in 0..30 {
        body.push_str(&format!("// commentary for item {i}\n"));
        body.push_str(&format!("// commentary continuation {i}\n"));
        body.push_str(&format!("fn f{i}() {{ let _ = {i}; }}\n"));
    }
    let report = compute_genome_report_fast(&body, Path::new("dense.rs"));
    assert!(
        report.parsimony.comment_ratio > 0.3,
        "expected comment_ratio > 0.3, got {}",
        report.parsimony.comment_ratio
    );
}

#[test]
fn tiny_function_fraction_records_for_predicate_extraction() {
    let mut body = String::new();
    for i in 0..15 {
        body.push_str(&format!("fn p{i}() -> bool {{ true }}\n"));
    }
    let report = compute_genome_report_fast(&body, Path::new("tinies.rs"));
    assert!(
        report.parsimony.tiny_fn_fraction > 0.5,
        "expected tiny_fn_fraction > 0.5, got {}",
        report.parsimony.tiny_fn_fraction
    );
}
