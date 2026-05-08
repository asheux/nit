use tree_sitter::Tree;

use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, compute_line_starts, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use super::structural::{seed_highlight_bytes, seed_highlight_to_value, seed_parse, SeedLanguage};

pub(crate) struct ComplexityFieldEncoder;

impl SeedEncoder for ComplexityFieldEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::ComplexityField
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 32usize;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();

        if bytes.is_empty() {
            return grid;
        }

        let line_count = input.text.lines().count().max(1);
        let parsed = seed_parse(input.text, input.file_path);

        let (nesting, complexity, entropy, uniqueness) = match &parsed {
            Some((tree, lang)) => (
                complexity_nesting_depth(tree, line_count),
                complexity_cyclomatic(tree, line_count),
                complexity_token_entropy(input.text, *lang, tree, line_count),
                complexity_identifier_uniqueness(tree, input.text, line_count),
            ),
            None => {
                // Without tree-sitter we cannot compute meaningful metrics, so
                // we produce a moderate signal rather than byte-level noise.
                let neutral = vec![128.0f32; line_count];
                (neutral.clone(), neutral.clone(), neutral.clone(), neutral)
            }
        };

        for gy in 0..size {
            let line = (gy * line_count / size).min(line_count.saturating_sub(1));
            let n = nesting.get(line).copied().unwrap_or(0.0);
            let c = complexity.get(line).copied().unwrap_or(0.0);
            let e = entropy.get(line).copied().unwrap_or(0.0);
            let u = uniqueness.get(line).copied().unwrap_or(0.0);

            let value = (n * 0.25 + c * 0.30 + e * 0.25 + u * 0.20).clamp(0.0, 255.0) as u8;

            for gx in 0..size {
                grid.set(gx, gy, value);
            }
        }

        if let Some((tree, lang)) = &parsed {
            blend_per_column_token_signal(&mut grid, input.text, *lang, tree, size, line_count);
        }

        normalize_grid(&mut grid);
        apply_structural_noise(&mut grid, size, seed_nonce, bytes, variant);
        grid
    }
}

fn blend_per_column_token_signal(
    grid: &mut SeedValueGrid,
    text: &str,
    lang: SeedLanguage,
    tree: &Tree,
    size: usize,
    line_count: usize,
) {
    let groups = seed_highlight_bytes(text, lang, tree);
    let line_starts = compute_line_starts(text);
    for gy in 0..size {
        let line = (gy * line_count / size).min(line_count.saturating_sub(1));
        let start = line_starts.get(line).copied().unwrap_or(0);
        let end = line_starts.get(line + 1).copied().unwrap_or(text.len());
        let line_len = end.saturating_sub(start).max(1);
        for gx in 0..size {
            let col = gx * line_len / size;
            let byte_idx = start + col.min(line_len.saturating_sub(1));
            if byte_idx < groups.len() {
                if let Some(group) = groups[byte_idx] {
                    let col_value = seed_highlight_to_value(Some(group));
                    let base = grid.get(gx, gy) as u16;
                    // Blend: 80% metrics, 20% column token.
                    let blended = ((base * 80 + col_value as u16 * 20) / 100).min(255) as u8;
                    grid.set(gx, gy, blended);
                }
            }
        }
    }
}

fn complexity_nesting_depth(tree: &Tree, line_count: usize) -> Vec<f32> {
    let mut per_line = vec![0u32; line_count];
    let mut stack = vec![(tree.root_node(), 0u32)];

    while let Some((node, depth)) = stack.pop() {
        let start_line = node.start_position().row;
        let end_line = node.end_position().row;
        for line in start_line..=end_line.min(line_count.saturating_sub(1)) {
            if line < per_line.len() {
                per_line[line] = per_line[line].max(depth);
            }
        }
        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                stack.push((child, depth + 1));
            }
        }
    }

    let max_depth = per_line.iter().copied().max().unwrap_or(0).max(1) as f32;
    per_line
        .iter()
        .map(|&d| (d as f32 / max_depth * 255.0).clamp(0.0, 255.0))
        .collect()
}

struct FuncInfo {
    start_line: usize,
    end_line: usize,
    complexity: u32,
}

fn complexity_cyclomatic(tree: &Tree, line_count: usize) -> Vec<f32> {
    let mut per_line = vec![0.0f32; line_count];
    let root = tree.root_node();
    let mut func_stack = vec![root];
    let mut max_complexity = 0u32;
    let mut functions: Vec<FuncInfo> = Vec::new();

    while let Some(node) = func_stack.pop() {
        let kind = node.kind();
        let is_func = matches!(
            kind,
            "function_item"
                | "function_definition"
                | "method_definition"
                | "function_declaration"
                | "arrow_function"
                | "closure_expression"
                | "lambda"
                | "decorated_definition"
        );

        if is_func {
            let complexity = count_decision_points(node);
            max_complexity = max_complexity.max(complexity);
            functions.push(FuncInfo {
                start_line: node.start_position().row,
                end_line: node.end_position().row,
                complexity,
            });
        }

        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                func_stack.push(child);
            }
        }
    }

    let max_complexity = max_complexity.max(1) as f32;
    for func in &functions {
        let score = (func.complexity as f32 / max_complexity * 255.0).clamp(0.0, 255.0);
        for line in func.start_line..=func.end_line.min(line_count.saturating_sub(1)) {
            if line < per_line.len() {
                per_line[line] = per_line[line].max(score);
            }
        }
    }

    per_line
}

fn count_decision_points(node: tree_sitter::Node) -> u32 {
    // Cyclomatic-complexity base of 1 + one per decision point.
    let mut count = 1u32;
    let mut stack = vec![node];

    while let Some(n) = stack.pop() {
        let kind = n.kind();
        let is_decision = matches!(
            kind,
            "if_expression"
                | "if_statement"
                | "elif_clause"
                | "match_expression"
                | "match_arm"
                | "while_expression"
                | "while_statement"
                | "for_expression"
                | "for_statement"
                | "for_in_statement"
                | "loop_expression"
                | "switch_case"
                | "ternary_expression"
                | "conditional_expression"
                | "except_clause"
                | "catch_clause"
        );
        if is_decision {
            count += 1;
        }

        if kind == "binary_expression" || kind == "boolean_operator" {
            if let Some(op) = n.child_by_field_name("operator") {
                let op_kind = op.kind();
                if op_kind == "&&" || op_kind == "||" || op_kind == "??" {
                    count += 1;
                }
            }
        }

        for i in 0..n.child_count() {
            if let Some(child) = n.child(i) {
                stack.push(child);
            }
        }
    }

    count
}

fn complexity_token_entropy(
    text: &str,
    language: SeedLanguage,
    tree: &Tree,
    line_count: usize,
) -> Vec<f32> {
    let groups = seed_highlight_bytes(text, language, tree);
    let line_starts = compute_line_starts(text);
    // Approximate distinct highlight categories. Used to normalize entropy
    // across files of vastly different vocabularies.
    let num_categories = 12.0f32;
    let max_entropy = num_categories.log2();
    let max_entropy = if max_entropy > 0.0 { max_entropy } else { 1.0 };

    let mut per_line = vec![0.0f32; line_count];

    for (line, per_line_val) in per_line.iter_mut().enumerate() {
        let start = line_starts.get(line).copied().unwrap_or(0);
        let end = line_starts.get(line + 1).copied().unwrap_or(text.len());
        if start >= end {
            continue;
        }

        let mut freq = [0u32; 16];
        let mut total = 0u32;
        for g in &groups[start..end.min(groups.len())] {
            let cat = match g {
                None => 0,
                Some(g) => (*g as u32 % 15) + 1,
            };
            freq[cat as usize] += 1;
            total += 1;
        }

        if total == 0 {
            continue;
        }

        let total_f = total as f32;
        let mut h = 0.0f32;
        for &f in &freq {
            if f > 0 {
                let p = f as f32 / total_f;
                h -= p * p.log2();
            }
        }

        *per_line_val = (h / max_entropy * 255.0).clamp(0.0, 255.0);
    }

    per_line
}

fn complexity_identifier_uniqueness(tree: &Tree, text: &str, line_count: usize) -> Vec<f32> {
    let mut per_line = vec![0.0f32; line_count];
    let source = text.as_bytes();
    let root = tree.root_node();
    let mut func_stack = vec![root];

    while let Some(node) = func_stack.pop() {
        let kind = node.kind();
        let is_scope = matches!(
            kind,
            "function_item"
                | "function_definition"
                | "method_definition"
                | "function_declaration"
                | "arrow_function"
                | "closure_expression"
                | "lambda"
                | "block"
        );

        if is_scope {
            let mut idents: Vec<&[u8]> = Vec::new();
            let mut id_stack = vec![node];
            while let Some(n) = id_stack.pop() {
                if n.kind() == "identifier" || n.kind() == "name" {
                    let start = n.start_byte();
                    let end = n.end_byte().min(source.len());
                    if end > start {
                        idents.push(&source[start..end]);
                    }
                }
                for i in 0..n.child_count() {
                    if let Some(child) = n.child(i) {
                        id_stack.push(child);
                    }
                }
            }

            let total = idents.len().max(1);
            let mut sorted = idents.clone();
            sorted.sort_unstable();
            sorted.dedup();
            let unique = sorted.len();
            let ratio = unique as f32 / total as f32;
            let score = (ratio * 255.0).clamp(0.0, 255.0);

            let start_line = node.start_position().row;
            let end_line = node.end_position().row;
            for line in start_line..=end_line.min(line_count.saturating_sub(1)) {
                if line < per_line.len() && score > per_line[line] {
                    per_line[line] = score;
                }
            }
        }

        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                func_stack.push(child);
            }
        }
    }

    per_line
}
