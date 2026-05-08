use std::collections::HashMap;

use crate::genome_report::{GenomeRecommendation, RecommendationSeverity};

use super::thresholds::{ENTROPY_LOW, ENTROPY_MIN_TOKENS, ENTROPY_WINDOW_LINES, RANGE_GAP};

pub(super) fn analyze(
    _text: &str,
    lines: &[&str],
    root: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    if lines.len() < ENTROPY_WINDOW_LINES {
        return;
    }

    let mut tokens_per_line: Vec<Vec<&str>> = vec![Vec::new(); lines.len()];
    collect_leaf_tokens(root, &mut tokens_per_line);

    let low_ranges = scan_low_entropy_windows(&tokens_per_line, lines.len());

    for (s, e, val) in low_ranges {
        let s1 = s + 1;
        let e1 = e;
        recs.push(GenomeRecommendation {
            metric: "token_entropy".into(),
            severity: RecommendationSeverity::Info,
            message: format!(
                "Lines {s1}-{e1} have low token diversity (entropy {val:.1}). \
                 This may indicate copy-paste code. Consider extracting a shared abstraction.",
            ),
            location: Some(format!("{s1}-{e1}")),
        });
    }
}

fn scan_low_entropy_windows(
    tokens_per_line: &[Vec<&str>],
    line_count: usize,
) -> Vec<(usize, usize, f32)> {
    let window = ENTROPY_WINDOW_LINES;
    let mut low_ranges: Vec<(usize, usize, f32)> = Vec::new();

    for start in 0..=line_count.saturating_sub(window) {
        let end = (start + window).min(line_count);
        let Some((entropy, total)) = window_entropy(&tokens_per_line[start..end]) else {
            continue;
        };
        if total < ENTROPY_MIN_TOKENS || entropy >= ENTROPY_LOW {
            continue;
        }
        match low_ranges.last_mut() {
            Some((_, prev_end, min_e)) if start <= *prev_end + RANGE_GAP => {
                *prev_end = end;
                if entropy < *min_e {
                    *min_e = entropy;
                }
            }
            _ => low_ranges.push((start, end, entropy)),
        }
    }
    low_ranges
}

fn window_entropy(window: &[Vec<&str>]) -> Option<(f32, usize)> {
    let mut kind_counts: HashMap<&str, usize> = HashMap::new();
    let mut total = 0usize;
    for line_tokens in window {
        for &kind in line_tokens {
            *kind_counts.entry(kind).or_insert(0) += 1;
            total += 1;
        }
    }
    if total == 0 {
        return None;
    }
    Some((shannon_entropy(&kind_counts, total), total))
}

fn collect_leaf_tokens<'a>(node: &tree_sitter::Node<'a>, out: &mut Vec<Vec<&'a str>>) {
    if node.child_count() == 0 {
        let line = node.start_position().row;
        if line < out.len() {
            out[line].push(node.kind());
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_leaf_tokens(&child, out);
    }
}

fn shannon_entropy(counts: &HashMap<&str, usize>, total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let t = total as f64;
    let mut entropy = 0.0f64;
    for &count in counts.values() {
        if count == 0 {
            continue;
        }
        let p = count as f64 / t;
        entropy -= p * p.log2();
    }
    entropy as f32
}
