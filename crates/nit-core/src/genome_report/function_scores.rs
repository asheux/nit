//! Per-function structural metrics walker. Builds `Vec<FunctionScore>`
//! from a parsed AST, ordered by cognitive complexity (worst first), and
//! surfaces the worst-offender as a targeted recommendation when its
//! cognitive complexity crosses the SonarSource "very hard to follow"
//! threshold (~25).

use std::path::Path;

use crate::seed::encoders::ast_features::{classify_role, seed_parse, RoleBand};

use super::{FunctionScore, GenomeRecommendation, RecommendationSeverity};

pub(super) fn compute_function_scores(text: &str, file_path: &Path) -> Vec<FunctionScore> {
    let Some((tree, _lang)) = seed_parse(text, Some(file_path)) else {
        return Vec::new();
    };

    let mut out: Vec<FunctionScore> = Vec::new();
    let mut stack: Vec<(tree_sitter::Node, u32, u8)> = vec![(tree.root_node(), 0, 0)];
    while let Some((node, depth, parent_cf_depth)) = stack.pop() {
        let kind = node.kind();
        let role_band = classify_role(kind);
        let child_cf_depth = if role_band == RoleBand::ControlFlow {
            parent_cf_depth.saturating_add(1)
        } else {
            parent_cf_depth
        };
        if is_function_kind(kind) {
            out.push(measure_function(node, kind));
        }
        let mut walker = node.walk();
        for child in node.children(&mut walker) {
            stack.push((child, depth + 1, child_cf_depth));
        }
    }
    out.sort_by(|a, b| b.cognitive.cmp(&a.cognitive));
    out
}

fn is_function_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_definition"
            | "method_definition"
            | "function_declaration"
            | "arrow_function"
            | "closure_expression"
            | "lambda"
            | "method"
            | "method_declaration"
    )
}

fn measure_function(root: tree_sitter::Node, kind: &str) -> FunctionScore {
    let mut node_count: u32 = 0;
    let mut max_depth: u8 = 0;
    let mut cognitive: u32 = 0;
    let mut cyclomatic: u32 = 0;
    let mut stack: Vec<(tree_sitter::Node, u8, u8)> = vec![(root, 0, 0)];
    while let Some((node, rel_depth, cf_depth)) = stack.pop() {
        if !node.is_named() {
            continue;
        }
        let kind_str = node.kind();
        let role_band = classify_role(kind_str);
        if role_band == RoleBand::ControlFlow {
            cyclomatic += 1;
            cognitive += 1 + cf_depth as u32;
        }
        node_count += 1;
        if rel_depth > max_depth {
            max_depth = rel_depth;
        }
        let next_cf = if role_band == RoleBand::ControlFlow {
            cf_depth.saturating_add(1)
        } else {
            cf_depth
        };
        let mut walker = node.walk();
        for child in node.children(&mut walker) {
            stack.push((child, rel_depth.saturating_add(1), next_cf));
        }
    }
    FunctionScore {
        kind: kind.to_string(),
        start_line: (root.start_position().row as u32).saturating_add(1),
        end_line: (root.end_position().row as u32).saturating_add(1),
        node_count,
        max_depth,
        cognitive,
        cyclomatic,
    }
}

pub(super) fn surface_top_offender_recommendation(
    function_scores: &[FunctionScore],
    recommendations: &mut Vec<GenomeRecommendation>,
) {
    let Some(worst) = function_scores.first() else {
        return;
    };
    const COGNITIVE_WARN: u32 = 25;
    if worst.cognitive < COGNITIVE_WARN {
        return;
    }
    recommendations.push(GenomeRecommendation {
        metric: "cognitive_complexity".into(),
        severity: RecommendationSeverity::Warning,
        message: format!(
            "Function at lines {}-{} has cognitive complexity {} ({} branches, max depth {}). Consider decomposing.",
            worst.start_line,
            worst.end_line,
            worst.cognitive,
            worst.cyclomatic,
            worst.max_depth,
        ),
        location: Some(format!("{}:{}", worst.start_line, worst.end_line)),
    });
}
