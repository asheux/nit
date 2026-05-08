use std::collections::HashSet;

use crate::genome_report::{GenomeRecommendation, RecommendationSeverity};

use super::thresholds::{CYCLOMATIC_CRITICAL, IDENTIFIER_UNIQUENESS_MIN};

pub(super) fn walk_top_level(
    text: &str,
    lines: &[&str],
    root: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let mut walker = root.walk();
    for top_node in root.children(&mut walker) {
        let kind = top_node.kind();
        if is_function_kind(kind) {
            analyze_function(text, lines, &top_node, recs);
        } else if kind == "impl_item" {
            walk_impl_methods(text, lines, &top_node, recs);
        }
    }
}

fn walk_impl_methods(
    text: &str,
    lines: &[&str],
    impl_node: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let mut walker = impl_node.walk();
    for member in impl_node.children(&mut walker) {
        if matches!(
            member.kind(),
            "function_item" | "function_definition" | "method_definition"
        ) {
            analyze_function(text, lines, &member, recs);
        }
    }
}

fn analyze_function(
    text: &str,
    _lines: &[&str],
    fn_node: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let fn_name = extract_function_name(text, fn_node).unwrap_or_else(|| "<anonymous>".to_string());
    let start_line = fn_node.start_position().row + 1;
    let end_line = fn_node.end_position().row + 1;
    let location = format!("{fn_name}:{start_line}-{end_line}");

    let cyclomatic = compute_cyclomatic_complexity(text, fn_node);
    if cyclomatic > CYCLOMATIC_CRITICAL {
        recs.push(GenomeRecommendation {
            metric: "cyclomatic_complexity".into(),
            severity: RecommendationSeverity::Critical,
            message: format!(
                "Split {fn_name}() (complexity {cyclomatic}) into smaller functions. \
                 Consider extracting logic in lines {start_line}-{end_line} into a separate function.",
            ),
            location: Some(location.clone()),
        });
    }

    let (total_idents, unique_idents) = count_identifiers(text, fn_node);
    if total_idents == 0 {
        return;
    }
    let uniqueness = unique_idents as f32 / total_idents as f32;
    if uniqueness >= IDENTIFIER_UNIQUENESS_MIN {
        return;
    }
    let pct = ((1.0 - uniqueness) * 100.0).round() as u32;
    recs.push(GenomeRecommendation {
        metric: "identifier_uniqueness".into(),
        severity: RecommendationSeverity::Warning,
        message: format!(
            "{pct}% of identifiers in {fn_name} are reused names. \
             Use descriptive names that reflect purpose.",
        ),
        location: Some(location),
    });
}

fn extract_function_name(text: &str, fn_node: &tree_sitter::Node<'_>) -> Option<String> {
    let mut walker = fn_node.walk();
    for child in fn_node.children(&mut walker) {
        if matches!(child.kind(), "name" | "identifier" | "property_identifier") {
            return Some(text[child.byte_range()].to_string());
        }
    }
    None
}

fn compute_cyclomatic_complexity(text: &str, fn_root: &tree_sitter::Node<'_>) -> u32 {
    let root_id = fn_root.id();
    let mut complexity = 1u32;
    let mut frontier = vec![*fn_root];
    while let Some(visiting) = frontier.pop() {
        complexity += branching_weight(text, &visiting);
        let mut walker = visiting.walk();
        for descendant in visiting.children(&mut walker) {
            if is_nested_function_kind(descendant.kind()) && descendant.id() != root_id {
                continue;
            }
            frontier.push(descendant);
        }
    }
    complexity
}

fn branching_weight(text: &str, node: &tree_sitter::Node<'_>) -> u32 {
    match node.kind() {
        "if_expression"
        | "if_statement"
        | "match_expression"
        | "match_statement"
        | "while_expression"
        | "while_statement"
        | "for_expression"
        | "for_statement"
        | "for_in_statement"
        | "loop_expression"
        | "conditional_expression"
        | "ternary_expression" => 1,
        "binary_expression" => {
            let op = node
                .child_by_field_name("operator")
                .map(|operator| &text[operator.byte_range()]);
            u32::from(matches!(op, Some("&&" | "||")))
        }
        _ => 0,
    }
}

fn count_identifiers(text: &str, fn_root: &tree_sitter::Node<'_>) -> (usize, usize) {
    let mut names: Vec<String> = Vec::new();
    let mut frontier = vec![*fn_root];
    while let Some(visiting) = frontier.pop() {
        if visiting.kind() == "identifier" {
            names.push(text[visiting.byte_range()].to_string());
        }
        let mut walker = visiting.walk();
        for descendant in visiting.children(&mut walker) {
            frontier.push(descendant);
        }
    }
    let total = names.len();
    let unique: HashSet<String> = names.into_iter().collect();
    (total, unique.len())
}

fn is_function_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_definition"
            | "arrow_function"
    )
}

fn is_nested_function_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_definition"
            | "arrow_function"
            | "closure_expression"
    )
}
