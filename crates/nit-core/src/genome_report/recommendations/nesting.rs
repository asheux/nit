use std::collections::HashMap;

use crate::genome_report::{GenomeRecommendation, RecommendationSeverity};

use super::thresholds::{NESTING_DEPTH_WARN, RANGE_GAP};

pub(super) fn analyze(
    _text: &str,
    root: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let mut max_depth_per_line: HashMap<usize, usize> = HashMap::new();
    collect_depth(root, 0, &mut max_depth_per_line);

    let flagged = sorted_flagged_lines(&max_depth_per_line);
    if flagged.is_empty() {
        return;
    }

    for (s, e, d) in coalesce_ranges(&flagged) {
        let s1 = s + 1;
        let e1 = e + 1;
        recs.push(GenomeRecommendation {
            metric: "nesting_depth".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Flatten nesting in lines {s1}-{e1} (depth {d}). \
                 Consider early returns, guard clauses, or extracting the inner block.",
            ),
            location: Some(format!("{s1}-{e1}")),
        });
    }
}

fn sorted_flagged_lines(per_line: &HashMap<usize, usize>) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = per_line
        .iter()
        .filter(|&(_, &d)| d > NESTING_DEPTH_WARN)
        .map(|(&line, &depth)| (line, depth))
        .collect();
    out.sort_by_key(|&(line, _)| line);
    out
}

fn coalesce_ranges(sorted: &[(usize, usize)]) -> Vec<(usize, usize, usize)> {
    let mut ranges: Vec<(usize, usize, usize)> = Vec::new();
    let (mut start, first_depth) = sorted[0];
    let mut end = start;
    let mut max_d = first_depth;
    for &(line, depth) in sorted.iter().skip(1) {
        if line <= end + RANGE_GAP {
            end = line;
            max_d = max_d.max(depth);
        } else {
            ranges.push((start, end, max_d));
            start = line;
            end = line;
            max_d = depth;
        }
    }
    ranges.push((start, end, max_d));
    ranges
}

fn collect_depth(node: &tree_sitter::Node<'_>, depth: usize, out: &mut HashMap<usize, usize>) {
    let start_line = node.start_position().row;
    let end_line = node.end_position().row;
    for line in start_line..=end_line {
        let entry = out.entry(line).or_insert(0);
        *entry = (*entry).max(depth);
    }
    let mut cursor = node.walk();
    let child_depth = if is_nesting_node(node.kind()) {
        depth + 1
    } else {
        depth
    };
    for child in node.children(&mut cursor) {
        collect_depth(&child, child_depth, out);
    }
}

fn is_nesting_node(kind: &str) -> bool {
    matches!(
        kind,
        "block"
            | "if_expression"
            | "if_statement"
            | "else_clause"
            | "match_expression"
            | "match_statement"
            | "while_expression"
            | "while_statement"
            | "for_expression"
            | "for_statement"
            | "for_in_statement"
            | "loop_expression"
            | "try_statement"
            | "catch_clause"
    )
}
