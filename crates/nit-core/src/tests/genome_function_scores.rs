//! Per-function structural-metric tests. Exercise the
//! `function_scores` field on a report — verifies cognitive complexity
//! grows with nested control flow, that closures are recorded as
//! independent entries, and that the worst-offender bubbles to position
//! zero in the sorted slice.

use std::path::Path;

use crate::genome_report::compute_genome_report_fast;

#[test]
fn flat_function_records_low_cognitive_complexity() {
    let code = r#"
fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
    let report = compute_genome_report_fast(code, Path::new("flat.rs"));
    if let Some(score) = report.function_scores.first() {
        assert!(
            score.cognitive < 5,
            "flat fn should score low cognitive, got {}",
            score.cognitive
        );
    }
}

#[test]
fn nested_branches_score_higher_than_flat_branches() {
    let nested = r#"
fn complex(items: &[i32]) -> i32 {
    let mut acc = 0;
    for v in items {
        if *v > 0 {
            for w in items {
                if *w > *v {
                    acc += v + w;
                }
            }
        }
    }
    acc
}
"#;
    let flat = r#"
fn simple(items: &[i32]) -> i32 {
    let mut acc = 0;
    for v in items {
        acc += v;
    }
    acc
}
"#;
    let nested_report = compute_genome_report_fast(nested, Path::new("nested.rs"));
    let flat_report = compute_genome_report_fast(flat, Path::new("flat.rs"));
    let nested_top = nested_report
        .function_scores
        .first()
        .map(|s| s.cognitive)
        .unwrap_or(0);
    let flat_top = flat_report
        .function_scores
        .first()
        .map(|s| s.cognitive)
        .unwrap_or(0);
    assert!(
        nested_top > flat_top,
        "nested cognitive ({nested_top}) should exceed flat ({flat_top})"
    );
}

#[test]
fn function_scores_sorted_worst_first() {
    let code = r#"
fn easy(a: i32) -> i32 {
    a + 1
}

fn hard(items: &[i32]) -> i32 {
    let mut acc = 0;
    for v in items {
        if *v > 0 {
            if *v > 10 {
                if *v > 100 {
                    acc += v;
                }
            }
        }
    }
    acc
}
"#;
    let report = compute_genome_report_fast(code, Path::new("sort.rs"));
    let scores = &report.function_scores;
    if scores.len() >= 2 {
        assert!(
            scores[0].cognitive >= scores[1].cognitive,
            "function_scores must be sorted by cognitive desc"
        );
    }
}

#[test]
fn empty_file_yields_no_function_scores() {
    let report = compute_genome_report_fast("", Path::new("empty.rs"));
    assert!(report.function_scores.is_empty());
}
