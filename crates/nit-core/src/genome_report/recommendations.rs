use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::{Parser, Tree};

use crate::seed::SeedEncoderId;

use super::outlier::analyze_structural_outlier;
use super::{EncoderScore, GenomeRecommendation, RecommendationSeverity};

pub(super) fn ts_parse(text: &str, file_path: &Path) -> Option<Tree> {
    let ext = file_path.extension()?.to_str()?;
    let language = match ext {
        "rs" => tree_sitter_rust::language(),
        "py" => tree_sitter_python::language(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::language(),
        "ts" | "tsx" => tree_sitter_typescript::language_typescript(),
        "html" | "htm" => tree_sitter_html::language(),
        "css" => tree_sitter_css::language(),
        "json" => tree_sitter_json::language(),
        "toml" => tree_sitter_toml::language(),
        "sh" | "bash" => tree_sitter_bash::language(),
        _ => return None,
    };
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    parser.parse(text, None)
}

pub fn generate_recommendations(
    text: &str,
    file_path: &Path,
    scores: &[EncoderScore],
) -> Vec<GenomeRecommendation> {
    let mut recs = Vec::new();

    // Density: low density means poor structural variety; high density is good.
    for score in scores {
        if matches!(
            score.encoder,
            SeedEncoderId::TokenSpectrum
                | SeedEncoderId::AstStructure
                | SeedEncoderId::ComplexityField
        ) && score.density < 0.15
        {
            recs.push(GenomeRecommendation {
                metric: "density".into(),
                severity: RecommendationSeverity::Warning,
                message: format!(
                    "{} density is {:.2}. The token-role distribution lacks variety. \
                     Mix different token types: keywords, operators, identifiers, types, and literals. \
                     Break up uniform code blocks with varied function shapes.",
                    score.encoder.label(),
                    score.density,
                ),
                location: None,
            });
        }
    }

    if let Some(ast_score) = scores
        .iter()
        .find(|s| s.encoder == SeedEncoderId::AstStructure)
    {
        if ast_score.components < 3 {
            recs.push(GenomeRecommendation {
                metric: "components".into(),
                severity: RecommendationSeverity::Warning,
                message: format!(
                    "AST Structure shows {} components. The code is monolithic. \
                     Consider splitting into multiple functions or modules with clear boundaries.",
                    ast_score.components,
                ),
                location: None,
            });
        }
    }

    // Structural encoder is the most common bottleneck. It operates at the raw
    // byte level; detect when it's an outlier and provide specific guidance.
    analyze_structural_outlier(text, scores, &mut recs);

    let tree = match ts_parse(text, file_path) {
        Some(t) => t,
        None => return recs,
    };

    let lines: Vec<&str> = text.lines().collect();
    let root = tree.root_node();

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        let kind = child.kind();
        let is_fn = matches!(
            kind,
            "function_item"
                | "function_definition"
                | "function_declaration"
                | "method_definition"
                | "arrow_function"
                | "impl_item"
        );
        if !is_fn {
            // Items inside impl blocks (and equivalents).
            let mut inner = child.walk();
            for grandchild in child.children(&mut inner) {
                if matches!(
                    grandchild.kind(),
                    "function_item" | "function_definition" | "method_definition"
                ) {
                    analyze_function_node(text, &lines, &grandchild, &mut recs);
                }
            }
            continue;
        }
        analyze_function_node(text, &lines, &child, &mut recs);
    }

    analyze_nesting_depth(text, &root, &mut recs);
    analyze_token_entropy(text, &lines, &root, &mut recs);

    recs
}

fn analyze_function_node(
    text: &str,
    _lines: &[&str],
    node: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let fn_name = find_function_name(text, node).unwrap_or_else(|| "<anonymous>".to_string());
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    let cc = compute_cyclomatic_complexity(text, node);
    if cc > 10 {
        recs.push(GenomeRecommendation {
            metric: "cyclomatic_complexity".into(),
            severity: RecommendationSeverity::Critical,
            message: format!(
                "Split {fn_name}() (complexity {cc}) into smaller functions. \
                 Consider extracting logic in lines {start_line}-{end_line} into a separate function.",
            ),
            location: Some(format!("{fn_name}:{start_line}-{end_line}")),
        });
    }

    let (total_ids, unique_ids) = count_identifiers(text, node);
    if total_ids > 0 {
        let uniqueness = unique_ids as f32 / total_ids as f32;
        if uniqueness < 0.5 {
            let pct = ((1.0 - uniqueness) * 100.0).round() as u32;
            recs.push(GenomeRecommendation {
                metric: "identifier_uniqueness".into(),
                severity: RecommendationSeverity::Warning,
                message: format!(
                    "{pct}% of identifiers in {fn_name} are reused names. \
                     Use descriptive names that reflect purpose.",
                ),
                location: Some(format!("{fn_name}:{start_line}-{end_line}")),
            });
        }
    }
}

fn find_function_name(text: &str, node: &tree_sitter::Node<'_>) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "name"
            || child.kind() == "identifier"
            || child.kind() == "property_identifier"
        {
            return Some(text[child.byte_range()].to_string());
        }
    }
    None
}

fn compute_cyclomatic_complexity(text: &str, node: &tree_sitter::Node<'_>) -> u32 {
    let mut cc = 1u32;
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        match n.kind() {
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
            | "ternary_expression" => {
                cc += 1;
            }
            "binary_expression" => {
                let op_text = n
                    .child_by_field_name("operator")
                    .map(|op| &text[op.byte_range()]);
                if matches!(op_text, Some("&&" | "||")) {
                    cc += 1;
                }
            }
            _ => {}
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            // Don't recurse into nested function definitions.
            if matches!(
                child.kind(),
                "function_item"
                    | "function_definition"
                    | "function_declaration"
                    | "method_definition"
                    | "arrow_function"
                    | "closure_expression"
            ) && child.id() != node.id()
            {
                continue;
            }
            stack.push(child);
        }
    }
    cc
}

fn count_identifiers(text: &str, node: &tree_sitter::Node<'_>) -> (usize, usize) {
    let mut all = Vec::new();
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier" {
            all.push(text[n.byte_range()].to_string());
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    let total = all.len();
    let unique: HashSet<_> = all.into_iter().collect();
    (total, unique.len())
}

fn analyze_nesting_depth(
    _text: &str,
    root: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let mut max_depth_per_line: HashMap<usize, usize> = HashMap::new();
    collect_depth(root, 0, &mut max_depth_per_line);

    let mut sorted_lines: Vec<(usize, usize)> = max_depth_per_line
        .iter()
        .filter(|&(_, &d)| d > 4)
        .map(|(&line, &depth)| (line, depth))
        .collect();
    sorted_lines.sort_by_key(|&(line, _)| line);

    if sorted_lines.is_empty() {
        return;
    }

    let mut ranges: Vec<(usize, usize, usize)> = Vec::new();
    let mut start = sorted_lines[0].0;
    let mut end = start;
    let mut max_d = sorted_lines[0].1;
    for &(line, depth) in sorted_lines.iter().skip(1) {
        if line <= end + 2 {
            // Contiguous with a 1-line gap allowance.
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

    for (s, e, d) in ranges {
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

fn analyze_token_entropy(
    _text: &str,
    lines: &[&str],
    root: &tree_sitter::Node<'_>,
    recs: &mut Vec<GenomeRecommendation>,
) {
    if lines.len() < 10 {
        return;
    }

    let mut tokens_per_line: Vec<Vec<&str>> = vec![Vec::new(); lines.len()];
    collect_leaf_tokens(root, &mut tokens_per_line);

    let window = 10;
    let mut low_ranges: Vec<(usize, usize, f32)> = Vec::new();

    for start in 0..=lines.len().saturating_sub(window) {
        let end = (start + window).min(lines.len());
        let mut kind_counts: HashMap<&str, usize> = HashMap::new();
        let mut total = 0usize;
        for line_tokens in &tokens_per_line[start..end] {
            for &kind in line_tokens {
                *kind_counts.entry(kind).or_insert(0) += 1;
                total += 1;
            }
        }
        if total < 5 {
            continue;
        }
        let entropy = shannon_entropy(&kind_counts, total);
        if entropy < 3.0 {
            match low_ranges.last_mut() {
                Some((_, ref mut prev_end, ref mut min_e)) if start <= *prev_end + 2 => {
                    *prev_end = end;
                    if entropy < *min_e {
                        *min_e = entropy;
                    }
                }
                _ => {
                    low_ranges.push((start, end, entropy));
                }
            }
        }
    }

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
