use std::path::Path;

use super::recommendations::ts_parse;
use super::{GenomeRecommendation, ParsimonyInfo, RecommendationSeverity};

/// Minimum significant lines before parsimony analysis is applied.
/// Below this threshold, files are too small for meaningful bloat detection.
pub(super) const PARSIMONY_MIN_LINES: usize = 40;

/// Functions averaging fewer than this many significant lines, combined with
/// a high function count, indicate over-splitting.
pub(super) const PARSIMONY_AVG_FN_BODY_THRESHOLD: f32 = 3.0;

/// Files with fewer functions than this are not considered over-split regardless
/// of average body size.
pub(super) const PARSIMONY_MIN_FN_COUNT: usize = 15;

/// A function body with this many significant lines or fewer is considered
/// "tiny" for the tiny-function-fraction check.
pub(super) const PARSIMONY_TINY_FN_LINES: usize = 5;

/// Predicate over-extraction / stub duplication threshold (>= 50% tiny fns).
pub(super) const PARSIMONY_TINY_FN_FRACTION_THRESHOLD: f32 = 0.50;

/// Set at 12 to avoid false positives on small structs with one-liner
/// accessor methods (a struct with 10 methods is normal; 12+ tiny functions
/// in a single file suggests predicate over-extraction).
pub(super) const PARSIMONY_TINY_FN_MIN_COUNT: usize = 12;

/// Well-documented code typically sits at 15-25%. Above 40% suggests
/// comments are being added to diversify the token stream for genome scores
/// rather than to explain non-obvious logic.
pub(super) const PARSIMONY_COMMENT_RATIO_THRESHOLD: f32 = 0.40;

/// Any occurrence of consecutive identical non-blank comment lines is a
/// duplicate-doc accident (a merge artifact or a refactor that forgot to
/// dedupe). Legitimate comments carry new information; a repeat never does.
pub(super) const PARSIMONY_DUPLICATE_COMMENT_THRESHOLD: usize = 1;

/// True for a trimmed line that is syntactically a comment or comment
/// continuation. The `*` rules are narrow on purpose: a bare
/// `starts_with('*')` would misclassify real code like `*ptr = 5` or
/// `*mut T = ...` as a comment, which both undercounts code lines AND
/// inflates the comment ratio enough to wrongly flag a file as
/// comment-padded. Block-comment continuation lines in practice are
/// `* text`, a bare `*`, or `*/`.
pub(super) fn is_comment_line(t: &str) -> bool {
    t.starts_with("//")
        || t.starts_with("/*")
        || t == "*"
        || t.starts_with("* ")
        || t.starts_with("*/")
}

pub(super) fn compute_parsimony(
    text: &str,
    file_path: &Path,
    significant_lines: usize,
) -> ParsimonyInfo {
    let tree = match ts_parse(text, file_path) {
        Some(t) => t,
        None => return ParsimonyInfo::default(),
    };

    let root = tree.root_node();
    let mut fn_body_sizes: Vec<usize> = Vec::new();
    let mut top_level_items: usize = 0;

    count_items_recursive(&root, text, 0, &mut fn_body_sizes, &mut top_level_items);

    let fn_count = fn_body_sizes.len();
    let fn_body_lines_total: usize = fn_body_sizes.iter().sum();
    let avg_fn_body_lines = if fn_count > 0 {
        fn_body_lines_total as f32 / fn_count as f32
    } else {
        0.0
    };
    let tiny_fn_fraction = if fn_count > 0 {
        let tiny = fn_body_sizes
            .iter()
            .filter(|&&s| s <= PARSIMONY_TINY_FN_LINES)
            .count();
        tiny as f32 / fn_count as f32
    } else {
        0.0
    };

    let item_density = if significant_lines > 0 {
        top_level_items as f32 / significant_lines as f32 * 100.0
    } else {
        0.0
    };

    // Scan for consecutive-duplicate line comments (// / ///) — each repeat
    // of a non-blank comment after an identical prior comment counts once.
    // Blank doc dividers (`//`, `///`) and any non-comment line break the
    // chain so repetition across code blocks isn't falsely counted.
    let mut comment_lines: usize = 0;
    let mut non_blank_lines: usize = 0;
    let mut duplicate_comment_lines: usize = 0;
    let mut prev_nonblank_comment: Option<&str> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        non_blank_lines += 1;
        if is_comment_line(t) {
            comment_lines += 1;
            if t.starts_with("//") {
                let content = t.trim_start_matches('/').trim_start_matches('!').trim();
                if content.is_empty() {
                    prev_nonblank_comment = None;
                } else {
                    if prev_nonblank_comment == Some(t) {
                        duplicate_comment_lines += 1;
                    }
                    prev_nonblank_comment = Some(t);
                }
            } else {
                prev_nonblank_comment = None;
            }
        } else {
            prev_nonblank_comment = None;
        }
    }
    let comment_ratio = if non_blank_lines > 0 {
        comment_lines as f32 / non_blank_lines as f32
    } else {
        0.0
    };

    let over_split = significant_lines >= PARSIMONY_MIN_LINES
        && fn_count >= PARSIMONY_MIN_FN_COUNT
        && avg_fn_body_lines > 0.0
        && avg_fn_body_lines < PARSIMONY_AVG_FN_BODY_THRESHOLD;

    // For comment padding, use non_blank_lines (not significant_lines) as the
    // minimum — the whole point of comment padding is that it inflates total
    // lines while keeping significant (code-only) lines low.
    let comment_padded =
        non_blank_lines >= PARSIMONY_MIN_LINES && comment_ratio > PARSIMONY_COMMENT_RATIO_THRESHOLD;

    // Catches predicate over-extraction and stub duplication even when the
    // average body size is pulled up by a few larger functions (e.g. 10
    // two-liners + 2 fifty-liners → avg ~12, but 83% of functions are tiny).
    let too_many_tiny = fn_count >= PARSIMONY_TINY_FN_MIN_COUNT
        && tiny_fn_fraction > PARSIMONY_TINY_FN_FRACTION_THRESHOLD;

    let duplicate_comments_flagged =
        duplicate_comment_lines >= PARSIMONY_DUPLICATE_COMMENT_THRESHOLD;

    let bloat_detected =
        over_split || comment_padded || too_many_tiny || duplicate_comments_flagged;

    ParsimonyInfo {
        fn_count,
        avg_fn_body_lines,
        item_density,
        comment_ratio,
        tiny_fn_fraction,
        duplicate_comment_lines,
        bloat_detected,
    }
}

fn count_items_recursive(
    node: &tree_sitter::Node<'_>,
    text: &str,
    depth: usize,
    fn_body_sizes: &mut Vec<usize>,
    top_level_items: &mut usize,
) {
    let kind = node.kind();

    let is_item = matches!(
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
    );

    if is_item && depth <= 1 {
        *top_level_items += 1;
    }

    let is_fn = matches!(
        kind,
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_definition"
            | "arrow_function"
    );

    if is_fn {
        let start = node.start_position().row;
        let end = node.end_position().row;
        let body_sig_lines = text
            .lines()
            .skip(start)
            .take(end.saturating_sub(start) + 1)
            .filter(|line| {
                let t = line.trim();
                !t.is_empty() && !is_comment_line(t) && t != "{" && t != "}"
            })
            .count();
        fn_body_sizes.push(body_sig_lines);
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
        // Don't double-count: skip nested function definitions inside
        // functions. Closures don't match our function kinds, so they
        // pass through.
        if is_fn
            && matches!(
                child.kind(),
                "function_item" | "function_definition" | "function_declaration"
            )
        {
            continue;
        }
        count_items_recursive(&child, text, child_depth, fn_body_sizes, top_level_items);
    }
}

pub(super) fn generate_parsimony_recommendations(
    parsimony: &ParsimonyInfo,
    recs: &mut Vec<GenomeRecommendation>,
) {
    let over_split = parsimony.fn_count >= PARSIMONY_MIN_FN_COUNT
        && parsimony.avg_fn_body_lines > 0.0
        && parsimony.avg_fn_body_lines < PARSIMONY_AVG_FN_BODY_THRESHOLD;

    if over_split {
        recs.push(GenomeRecommendation {
            metric: "parsimony".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Over-engineered: {} functions averaging {:.1} lines each. \
                 Tier capped at IV (Methuselah). Consolidate trivially small \
                 functions — merge related logic instead of splitting into \
                 many tiny functions to inflate genome scores.",
                parsimony.fn_count, parsimony.avg_fn_body_lines,
            ),
            location: None,
        });
    } else if parsimony.fn_count >= 10
        && parsimony.avg_fn_body_lines > 0.0
        && parsimony.avg_fn_body_lines < PARSIMONY_AVG_FN_BODY_THRESHOLD
    {
        recs.push(GenomeRecommendation {
            metric: "parsimony".into(),
            severity: RecommendationSeverity::Info,
            message: format!(
                "Functions average {:.1} lines. Consider whether some can be \
                 consolidated — small focused functions are good, but over-splitting \
                 simple logic adds complexity without improving quality.",
                parsimony.avg_fn_body_lines,
            ),
            location: None,
        });
    }

    if parsimony.comment_ratio > PARSIMONY_COMMENT_RATIO_THRESHOLD {
        recs.push(GenomeRecommendation {
            metric: "comment_padding".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Comment padding detected: {:.0}% of non-blank lines are comments. \
                 Tier capped at IV (Methuselah). Comments improve genome token \
                 diversity scores but adding them to game the system is penalized. \
                 Keep doc comments on public API items; remove trivial or redundant \
                 comments on private helpers and obvious logic.",
                parsimony.comment_ratio * 100.0,
            ),
            location: None,
        });
    } else if parsimony.comment_ratio > 0.30 {
        recs.push(GenomeRecommendation {
            metric: "comment_padding".into(),
            severity: RecommendationSeverity::Info,
            message: format!(
                "Comment ratio is {:.0}%. Approaching the 40% parsimony threshold. \
                 Ensure comments explain non-obvious logic rather than restating code.",
                parsimony.comment_ratio * 100.0,
            ),
            location: None,
        });
    }

    if parsimony.duplicate_comment_lines >= PARSIMONY_DUPLICATE_COMMENT_THRESHOLD {
        recs.push(GenomeRecommendation {
            metric: "duplicate_comments".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Found {} consecutive-duplicate comment line(s). Tier capped at \
                 IV (Methuselah). A repeated comment never adds information — \
                 delete the duplicate.",
                parsimony.duplicate_comment_lines,
            ),
            location: None,
        });
    }

    let too_many_tiny = parsimony.fn_count >= PARSIMONY_TINY_FN_MIN_COUNT
        && parsimony.tiny_fn_fraction > PARSIMONY_TINY_FN_FRACTION_THRESHOLD;

    if too_many_tiny {
        let tiny_count = (parsimony.tiny_fn_fraction * parsimony.fn_count as f32).round() as usize;
        recs.push(GenomeRecommendation {
            metric: "tiny_functions".into(),
            severity: RecommendationSeverity::Warning,
            message: format!(
                "Predicate over-extraction: {tiny_count} of {} functions have \
                 <= {PARSIMONY_TINY_FN_LINES} significant lines ({:.0}%). Tier capped \
                 at IV (Methuselah). Inline trivial predicates, combine related \
                 checks into single functions, and use macros for repetitive stubs \
                 instead of copy-pasting function bodies.",
                parsimony.fn_count,
                parsimony.tiny_fn_fraction * 100.0,
            ),
            location: None,
        });
    }
}
