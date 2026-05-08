use std::path::Path;

use super::source_scan::{is_comment_line, ts_parse};
use super::{GenomeRecommendation, ParsimonyInfo, RecommendationSeverity};

/// Tunable thresholds for the parsimony bloat detector. Each field has a
/// dedicated WHY rationale below — these numbers were tuned against real-world
/// false-positive cases (`state/multipane.rs`, accessor-heavy structs) and
/// should not be moved without re-validating.
pub(super) struct ParsimonyThresholds {
    /// Min significant lines before parsimony analysis runs at all. Below this,
    /// files are too small for meaningful bloat detection.
    pub min_lines: usize,
    /// Avg fn body lines below which the over-split heuristic fires (when
    /// combined with a high enough fn count).
    pub avg_fn_body_threshold: f32,
    /// Files with fewer functions than this are not considered over-split,
    /// regardless of average body size.
    pub min_fn_count: usize,
    /// A function body with this many significant lines or fewer is "tiny" for
    /// the tiny-fn-fraction check.
    pub tiny_fn_lines: usize,
    /// Tiny-fn-fraction threshold. Catches predicate over-extraction / stub
    /// duplication even when avg body size is pulled up by a few large fns.
    pub tiny_fn_fraction_threshold: f32,
    /// Set at 12 to avoid false positives on small structs with one-liner
    /// accessor methods (a struct with 10 methods is normal; 12+ tiny fns in
    /// a single file suggests predicate over-extraction).
    pub tiny_fn_min_count: usize,
    /// Well-documented code typically sits at 15-25%. Above 35% suggests the
    /// file has crossed from "well-documented" into "doc-comment heavy" —
    /// either padding for genome scores OR field-level comments that restate
    /// the field's purpose without adding non-obvious context. The 0.40
    /// threshold used to miss files like `state/multipane.rs` (37.44%) where
    /// every field carried a doc comment but the structural shape was still
    /// bloated; 0.35 catches that pattern with a wide margin to the
    /// next-densest production file (~22%).
    pub comment_ratio_threshold: f32,
    /// Any occurrence of consecutive identical non-blank comment lines is a
    /// duplicate-doc accident. Legitimate comments carry new information; a
    /// repeat never does.
    pub duplicate_comment_threshold: usize,
    /// Crosses from "well-factored" to "starting to over-split" — emits Info
    /// rather than the Warning that the >=15 / <3.0 combo emits.
    pub info_fn_count_threshold: usize,
    /// Approaching the comment-ratio threshold that warns; this lower bound
    /// emits an Info nudge first.
    pub comment_ratio_info_threshold: f32,
}

const THRESHOLDS: ParsimonyThresholds = ParsimonyThresholds {
    min_lines: 40,
    avg_fn_body_threshold: 3.0,
    min_fn_count: 15,
    tiny_fn_lines: 5,
    tiny_fn_fraction_threshold: 0.50,
    tiny_fn_min_count: 12,
    comment_ratio_threshold: 0.35,
    duplicate_comment_threshold: 1,
    info_fn_count_threshold: 10,
    comment_ratio_info_threshold: 0.30,
};

#[derive(Default)]
struct CommentScan {
    comment_lines: usize,
    non_blank_lines: usize,
    duplicate_comment_lines: usize,
}

pub(super) fn compute_parsimony(
    text: &str,
    file_path: &Path,
    significant_lines: usize,
) -> ParsimonyInfo {
    let Some(tree) = ts_parse(text, file_path) else {
        return ParsimonyInfo::default();
    };
    let root = tree.root_node();

    let mut fn_body_sizes: Vec<usize> = Vec::new();
    let mut top_level_items: usize = 0;
    count_items_recursive(&root, text, 0, &mut fn_body_sizes, &mut top_level_items);

    let fn_count = fn_body_sizes.len();
    let avg_fn_body_lines = mean_or_zero(&fn_body_sizes);
    let tiny_fn_fraction = tiny_fraction(&fn_body_sizes, THRESHOLDS.tiny_fn_lines);
    let item_density = if significant_lines > 0 {
        top_level_items as f32 / significant_lines as f32 * 100.0
    } else {
        0.0
    };

    let scan = scan_comments(text);
    let comment_ratio = if scan.non_blank_lines > 0 {
        scan.comment_lines as f32 / scan.non_blank_lines as f32
    } else {
        0.0
    };

    let info = ParsimonyInfo {
        fn_count,
        avg_fn_body_lines,
        item_density,
        comment_ratio,
        tiny_fn_fraction,
        duplicate_comment_lines: scan.duplicate_comment_lines,
        bloat_detected: false,
    };

    ParsimonyInfo {
        bloat_detected: detect_bloat(&info, significant_lines, scan.non_blank_lines),
        ..info
    }
}

fn mean_or_zero(sizes: &[usize]) -> f32 {
    if sizes.is_empty() {
        return 0.0;
    }
    sizes.iter().sum::<usize>() as f32 / sizes.len() as f32
}

fn tiny_fraction(sizes: &[usize], tiny_max: usize) -> f32 {
    if sizes.is_empty() {
        return 0.0;
    }
    let tiny = sizes.iter().filter(|&&s| s <= tiny_max).count();
    tiny as f32 / sizes.len() as f32
}

/// Count consecutive-duplicate line comments (`//` / `///`). Each repeat of a
/// non-blank comment after an identical prior comment counts once. Blank doc
/// dividers (`//`, `///`) and any non-comment line break the chain so
/// repetition across code blocks isn't falsely counted.
fn scan_comments(text: &str) -> CommentScan {
    let mut scan = CommentScan::default();
    let mut prev_nonblank_comment: Option<&str> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        scan.non_blank_lines += 1;
        if !is_comment_line(t) {
            prev_nonblank_comment = None;
            continue;
        }
        scan.comment_lines += 1;
        if !t.starts_with("//") {
            prev_nonblank_comment = None;
            continue;
        }
        let content = t.trim_start_matches('/').trim_start_matches('!').trim();
        if content.is_empty() {
            prev_nonblank_comment = None;
            continue;
        }
        if prev_nonblank_comment == Some(t) {
            scan.duplicate_comment_lines += 1;
        }
        prev_nonblank_comment = Some(t);
    }
    scan
}

fn detect_bloat(info: &ParsimonyInfo, significant_lines: usize, non_blank_lines: usize) -> bool {
    let over_split = significant_lines >= THRESHOLDS.min_lines
        && info.fn_count >= THRESHOLDS.min_fn_count
        && info.avg_fn_body_lines > 0.0
        && info.avg_fn_body_lines < THRESHOLDS.avg_fn_body_threshold;

    // For comment padding, use non_blank_lines (not significant_lines) as the
    // minimum — the whole point of comment padding is that it inflates total
    // lines while keeping significant (code-only) lines low.
    let comment_padded = non_blank_lines >= THRESHOLDS.min_lines
        && info.comment_ratio > THRESHOLDS.comment_ratio_threshold;

    let too_many_tiny = info.fn_count >= THRESHOLDS.tiny_fn_min_count
        && info.tiny_fn_fraction > THRESHOLDS.tiny_fn_fraction_threshold;

    let duplicate_comments = info.duplicate_comment_lines >= THRESHOLDS.duplicate_comment_threshold;

    over_split || comment_padded || too_many_tiny || duplicate_comments
}

fn count_items_recursive(
    node: &tree_sitter::Node<'_>,
    text: &str,
    depth: usize,
    fn_body_sizes: &mut Vec<usize>,
    top_level_items: &mut usize,
) {
    let kind = node.kind();

    if is_top_level_item_kind(kind) && depth <= 1 {
        *top_level_items += 1;
    }

    let is_fn = is_fn_kind(kind);
    if is_fn {
        fn_body_sizes.push(count_fn_body_lines(node, text));
    }

    // For impl/trait blocks, increment depth so their inner items are
    // counted at depth 1 (still top-level conceptually).
    let child_depth = if matches!(kind, "impl_item" | "trait_item" | "class_definition") {
        depth + 1
    } else {
        depth
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // Skip nested function definitions inside functions to avoid
        // double-counting; closures don't match these kinds and pass through.
        if is_fn && is_nested_fn_skip(child.kind()) {
            continue;
        }
        count_items_recursive(&child, text, child_depth, fn_body_sizes, top_level_items);
    }
}

fn is_top_level_item_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "struct_item"
            | "enum_item"
            | "type_item"
            | "trait_item"
            | "impl_item"
            | "const_item"
            | "static_item"
            | "class_definition"
            | "decorated_definition"
    )
}

fn is_fn_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_definition"
            | "arrow_function"
    )
}

fn is_nested_fn_skip(kind: &str) -> bool {
    matches!(
        kind,
        "function_item" | "function_definition" | "function_declaration"
    )
}

fn count_fn_body_lines(node: &tree_sitter::Node<'_>, text: &str) -> usize {
    let start = node.start_position().row;
    let end = node.end_position().row;
    text.lines()
        .skip(start)
        .take(end.saturating_sub(start) + 1)
        .filter(|line| {
            let t = line.trim();
            !t.is_empty() && !is_comment_line(t) && t != "{" && t != "}"
        })
        .count()
}

pub(super) fn generate_parsimony_recommendations(
    parsimony: &ParsimonyInfo,
    recs: &mut Vec<GenomeRecommendation>,
) {
    emit_over_split_rec(parsimony, recs);
    emit_comment_padding_rec(parsimony, recs);
    emit_duplicate_comments_rec(parsimony, recs);
    emit_tiny_fns_rec(parsimony, recs);
}

fn emit_over_split_rec(p: &ParsimonyInfo, recs: &mut Vec<GenomeRecommendation>) {
    let avg_below =
        p.avg_fn_body_lines > 0.0 && p.avg_fn_body_lines < THRESHOLDS.avg_fn_body_threshold;
    if !avg_below {
        return;
    }
    if p.fn_count >= THRESHOLDS.min_fn_count {
        recs.push(GenomeRecommendation {
            metric: "parsimony".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Over-engineered: {} functions averaging {:.1} lines each. \
                 Tier capped at IV (Methuselah). Consolidate trivially small \
                 functions — merge related logic instead of splitting into \
                 many tiny functions to inflate genome scores.",
                p.fn_count, p.avg_fn_body_lines,
            ),
            location: None,
        });
    } else if p.fn_count >= THRESHOLDS.info_fn_count_threshold {
        recs.push(GenomeRecommendation {
            metric: "parsimony".into(),
            severity: RecommendationSeverity::Info,
            message: format!(
                "Functions average {:.1} lines. Consider whether some can be \
                 consolidated — small focused functions are good, but over-splitting \
                 simple logic adds complexity without improving quality.",
                p.avg_fn_body_lines,
            ),
            location: None,
        });
    }
}

fn emit_comment_padding_rec(p: &ParsimonyInfo, recs: &mut Vec<GenomeRecommendation>) {
    if p.comment_ratio > THRESHOLDS.comment_ratio_threshold {
        recs.push(GenomeRecommendation {
            metric: "comment_padding".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Comment padding detected: {:.0}% of non-blank lines are comments. \
                 Tier capped at IV (Methuselah). Comments improve genome token \
                 diversity scores but adding them to game the system is penalized. \
                 Keep doc comments on public API items; remove trivial or redundant \
                 comments on private helpers and obvious logic.",
                p.comment_ratio * 100.0,
            ),
            location: None,
        });
    } else if p.comment_ratio > THRESHOLDS.comment_ratio_info_threshold {
        recs.push(GenomeRecommendation {
            metric: "comment_padding".into(),
            severity: RecommendationSeverity::Info,
            message: format!(
                "Comment ratio is {:.0}%. Approaching the 40% parsimony threshold. \
                 Ensure comments explain non-obvious logic rather than restating code.",
                p.comment_ratio * 100.0,
            ),
            location: None,
        });
    }
}

fn emit_duplicate_comments_rec(p: &ParsimonyInfo, recs: &mut Vec<GenomeRecommendation>) {
    if p.duplicate_comment_lines < THRESHOLDS.duplicate_comment_threshold {
        return;
    }
    recs.push(GenomeRecommendation {
        metric: "duplicate_comments".into(),
        severity: RecommendationSeverity::Warning,
        message: format!(
            "Found {} consecutive-duplicate comment line(s). Tier capped at \
             IV (Methuselah). A repeated comment never adds information — \
             delete the duplicate.",
            p.duplicate_comment_lines,
        ),
        location: None,
    });
}

fn emit_tiny_fns_rec(p: &ParsimonyInfo, recs: &mut Vec<GenomeRecommendation>) {
    let too_many_tiny = p.fn_count >= THRESHOLDS.tiny_fn_min_count
        && p.tiny_fn_fraction > THRESHOLDS.tiny_fn_fraction_threshold;
    if !too_many_tiny {
        return;
    }
    let tiny_count = (p.tiny_fn_fraction * p.fn_count as f32).round() as usize;
    let tiny_lines = THRESHOLDS.tiny_fn_lines;
    recs.push(GenomeRecommendation {
        metric: "tiny_functions".into(),
        severity: RecommendationSeverity::Warning,
        message: format!(
            "Predicate over-extraction: {tiny_count} of {} functions have \
             <= {tiny_lines} significant lines ({:.0}%). Tier capped at \
             IV (Methuselah). Inline trivial predicates, combine related \
             checks into single functions, and use macros for repetitive stubs \
             instead of copy-pasting function bodies.",
            p.fn_count,
            p.tiny_fn_fraction * 100.0,
        ),
        location: None,
    });
}
