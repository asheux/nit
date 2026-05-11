//! Canonical AST features shared by every quality encoder.
//!
//! ## Why this exists
//!
//! Before this module, each encoder read raw source text and tree-sitter
//! highlight bytes to build its grid. That made the score sensitive to
//! changes that do not affect program behaviour: adding `// foo` comments,
//! renaming identifiers, reflowing whitespace, padding doc-strings. Agents
//! ran into the genome retry loop, couldn't refactor (scope locked), and
//! discovered that sprinkling comments shifts the GoL seed onto a different
//! attractor — often a higher tier. Goodhart's law in 100 lines.
//!
//! This module replaces "source text" with a canonical feature vector
//! derived from the AST alone:
//!
//! - **`role_band`** — which family the node belongs to (declaration,
//!   control-flow, expression, statement, type, literal, other). Derived
//!   from the node's tree-sitter `kind()` string, never from token text.
//! - **`kind_weight`** — a 0-255 weight reused from `ast_node_class`
//!   (declarations heaviest, literals lightest). Same kind always maps to
//!   the same weight, regardless of identifier names.
//! - **`depth`** — AST nesting depth (0 = root).
//! - **`row`** — source row the node starts on, *only* used for grouping
//!   row-level metrics. Whitespace inside a row doesn't move it.
//!
//! Comments are explicitly excluded — `kind() == "line_comment"` /
//! `"block_comment"` / `"comment"` never enter the feature vector. Same for
//! whitespace / newlines (which aren't named nodes anyway).
//!
//! ## What's still leaky
//!
//! Function/item *reordering* still changes the walk order and therefore
//! the feature vector. We treat that as a real structural change, not
//! gaming — the agent reordering items is reorganising the program, not
//! sneaking past a metric. If that turns out to be exploitable too,
//! sorting nodes by kind before encoding would close it (at the cost of
//! losing positional information).

use std::hash::{Hash, Hasher};
use std::path::Path;

use tree_sitter::Tree;

use super::structural::{ast_node_class, seed_parse};

/// Identifier-free, whitespace-free, comment-free projection of an AST.
/// Every quality encoder consumes this — none read raw source text.
#[allow(dead_code)]
pub(crate) struct AstFeatures {
    /// Walk-order list of significant AST nodes. Excludes comments.
    pub(crate) nodes: Vec<AstNodeFeature>,
    /// Number of distinct rows that contain at least one significant node.
    /// Retained for diagnostics + future per-row encoders; the live
    /// encoders index node sequences directly instead because raw-row
    /// bucketing is sensitive to comment / blank-line shifts.
    pub(crate) significant_rows: usize,
    /// Aggregated per-row metrics. Same caveat as `significant_rows` —
    /// currently unused by the live encoders, kept for invariance tests
    /// and as a stable handle if a row-aware encoder reappears.
    pub(crate) rows: Vec<RowMetrics>,
    /// Stable hash of the structural feature triple `(kind_weight,
    /// role_band, depth)` per node. Replaces the byte-hash that drove
    /// `apply_structural_noise` so identifier / comment / whitespace
    /// changes can't perturb the noise either.
    pub(crate) feature_hash: u64,
}

pub(crate) struct AstNodeFeature {
    /// 0-255 weight from `ast_node_class`. Stable per AST kind.
    pub(crate) kind_weight: u8,
    /// Coarse semantic family. Used for diversity / entropy histograms.
    pub(crate) role_band: RoleBand,
    /// AST nesting depth (root = 0). Clamped to u8 — files rarely exceed
    /// 30 levels and tree-sitter's stack already caps at u32::MAX, so the
    /// cast saturates safely.
    pub(crate) depth: u8,
    /// Row index of the node's first byte. Used only for per-row metrics
    /// (nesting / cyclomatic). Row counting is collapsed to *significant
    /// rows* in `RowMetrics`, so a comment-only line doesn't shift this.
    pub(crate) row_raw: u32,
    /// Index of the depth-1 ancestor this node belongs to. Used to group
    /// nodes by "top-level item" so groups can be sorted by structural
    /// signature — closes the function-reorder lever (swapping two `fn`
    /// items at the top level changes source row order but not the *set*
    /// of items). `u32::MAX` for nodes at depth <= 1 (the items themselves
    /// and the root).
    pub(crate) top_level_idx: u32,
}

/// Coarse semantic family. Seven bands chosen to mirror the existing
/// `SeedHighlight` taxonomy without retaining its byte-token granularity.
/// Stays a `u8` under the hood so encoders can index histograms cheaply.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
#[repr(u8)]
pub(crate) enum RoleBand {
    Declaration = 0,
    ControlFlow = 1,
    Expression = 2,
    Statement = 3,
    Type = 4,
    Literal = 5,
    Other = 6,
}

pub(crate) const ROLE_BAND_COUNT: usize = 7;

impl RoleBand {
    pub(crate) fn as_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
pub(crate) struct RowMetrics {
    /// Max AST depth observed on this significant row.
    pub(super) max_depth: u8,
    /// Number of control-flow nodes on this row. Approximates per-row
    /// cyclomatic contribution: each `if` / `match arm` / `loop` adds one.
    pub(super) control_flow_count: u8,
}

/// Build the canonical features for a file. Returns `None` only when
/// tree-sitter cannot parse the file (unknown extension, parser missing).
/// Encoders treat `None` as "uniform / degenerate output" — they do **not**
/// fall back to byte-level processing.
pub(crate) fn compute_ast_features(text: &str, file_path: Option<&Path>) -> Option<AstFeatures> {
    let (tree, _lang) = seed_parse(text, file_path)?;
    Some(build_features(text, &tree))
}

fn build_features(text: &str, tree: &Tree) -> AstFeatures {
    let mut nodes = Vec::new();
    let mut significant_rows_set = std::collections::BTreeSet::new();
    let mut row_max_depth: std::collections::HashMap<u32, u8> = std::collections::HashMap::new();
    let mut row_cf_count: std::collections::HashMap<u32, u8> = std::collections::HashMap::new();

    // Iterative DFS. Recursion would hit pathological-tree stack overflow
    // on auto-generated code; the explicit stack keeps depth bounded only
    // by heap memory. The stack triple is `(node, depth, top_level_idx)`:
    // `top_level_idx` is the index of the depth-1 ancestor (the "top-level
    // item" — function, struct, etc.) this node belongs to, or `u32::MAX`
    // for nodes at depth 0/1. Tracked here so descendants can be grouped
    // by item after the walk, then groups sorted by signature to close
    // the function-reorder lever.
    let mut stack: Vec<(tree_sitter::Node, u32, u32)> = vec![(tree.root_node(), 0, u32::MAX)];
    let mut next_top_level_idx: u32 = 0;
    while let Some((node, depth, parent_top_level)) = stack.pop() {
        let kind = node.kind();
        // Skip the entire subtree of nodes that are either irrelevant for
        // structural quality (comments, doc comments) or trivial-to-add
        // gaming levers (attributes / decorators — `#[derive(...)]`,
        // `#[allow(...)]`, `@property`). Both classes change the AST
        // without changing program behaviour, so excluding them keeps the
        // score honest.
        if is_skipped_kind(kind) {
            continue;
        }
        // A depth-1 node IS a top-level item; assign it (and all its
        // descendants) a fresh index. Below depth 1, inherit from parent.
        let my_top_level_idx = if depth == 1 {
            let idx = next_top_level_idx;
            next_top_level_idx = next_top_level_idx.saturating_add(1);
            idx
        } else {
            parent_top_level
        };
        // Macros: emit one feature for the invocation itself, never
        // traverse into the token tree. `println!("any string you want")`
        // and `dbg!()` then contribute the *same* single node — closes
        // the macro-argument-stuffing lever. The macro_invocation node
        // itself still counts as an Expression (in `classify_role`), so
        // it isn't free either.
        let collapse_macro = is_macro_invocation_kind(kind);
        if !collapse_macro {
            let mut walker = node.walk();
            for child in node.children(&mut walker) {
                stack.push((child, depth + 1, my_top_level_idx));
            }
        }
        if !node.is_named() {
            continue;
        }
        let depth_u8 = depth.min(255) as u8;
        let row_u32 = node.start_position().row as u32;
        let role_band = classify_role(kind);
        let kind_weight = ast_node_class(kind);
        nodes.push(AstNodeFeature {
            kind_weight,
            role_band,
            depth: depth_u8,
            row_raw: row_u32,
            top_level_idx: my_top_level_idx,
        });
        significant_rows_set.insert(row_u32);
        let entry_max = row_max_depth.entry(row_u32).or_insert(0);
        *entry_max = (*entry_max).max(depth_u8);
        if role_band == RoleBand::ControlFlow {
            let entry_cf = row_cf_count.entry(row_u32).or_insert(0);
            *entry_cf = entry_cf.saturating_add(1);
        }
    }

    // Two-step sort:
    //   1. Within each top-level item, restore source order via
    //      `(row_raw, depth)`. The DFS stack pushes children in source
    //      order then pops in reverse, so a per-group source-order sort
    //      is required for deterministic encoder input.
    //   2. Group nodes by `top_level_idx` and reorder the groups by their
    //      structural signature (a stable hash of the group's feature
    //      triples). Swapping two `fn` items at the top level changes
    //      `row_raw` but not the signature, so the post-sort sequence is
    //      reorder-invariant.
    nodes.sort_by(|a, b| (a.row_raw, a.depth).cmp(&(b.row_raw, b.depth)));
    let nodes = sort_groups_by_signature(nodes);

    // Collapse raw row indices to dense "significant row" positions:
    // row 0 = first row with any significant node, etc. Insulates the
    // metrics from blank-line / comment-only rows.
    let significant_rows = significant_rows_set.len();
    let mut row_remap = std::collections::HashMap::with_capacity(significant_rows);
    for (idx, raw) in significant_rows_set.iter().enumerate() {
        row_remap.insert(*raw, idx);
    }
    let mut rows = vec![RowMetrics::default(); significant_rows.max(1)];
    for (raw, &md) in row_max_depth.iter() {
        if let Some(&idx) = row_remap.get(raw) {
            rows[idx].max_depth = md;
        }
    }
    for (raw, &cf) in row_cf_count.iter() {
        if let Some(&idx) = row_remap.get(raw) {
            rows[idx].control_flow_count = cf;
        }
    }

    let feature_hash = hash_features(&nodes, &rows);
    let _ = text; // text deliberately unused — invariant proof at compile time
    AstFeatures {
        nodes,
        significant_rows,
        rows,
        feature_hash,
    }
}

fn classify_role(kind: &str) -> RoleBand {
    if kind.contains("declaration")
        || kind.contains("definition")
        || kind == "function_item"
        || kind == "struct_item"
        || kind == "enum_item"
        || kind == "trait_item"
        || kind == "impl_item"
        || kind == "type_item"
        || kind == "module"
        || kind == "class_declaration"
        || kind == "interface_declaration"
    {
        return RoleBand::Declaration;
    }
    if kind.contains("if_")
        || kind.contains("match_")
        || kind.contains("switch_")
        || kind.contains("while_")
        || kind.contains("for_")
        || kind.contains("loop_")
        || kind.contains("try_")
        || kind.contains("catch_")
        || kind == "return_expression"
        || kind == "break_expression"
        || kind == "continue_expression"
    {
        return RoleBand::ControlFlow;
    }
    if kind.contains("expression")
        || kind.contains("call_")
        || kind.contains("binary_")
        || kind.contains("unary_")
        || kind.contains("assignment")
    {
        return RoleBand::Expression;
    }
    if kind.contains("statement")
        || kind.contains("block")
        || kind == "source_file"
        || kind == "program"
    {
        return RoleBand::Statement;
    }
    if kind.contains("type") || kind.contains("parameter") || kind.contains("argument") {
        return RoleBand::Type;
    }
    if kind.contains("literal")
        || kind.contains("string")
        || kind.contains("number")
        || kind == "identifier"
    {
        return RoleBand::Literal;
    }
    RoleBand::Other
}

fn is_comment_kind(kind: &str) -> bool {
    kind == "line_comment" || kind == "block_comment" || kind == "comment" || kind == "doc_comment"
}

/// Nodes whose entire subtree should be ignored. Currently: comments
/// (no behaviour, no structural signal) and attributes / decorators
/// (`#[derive(...)]`, `#[allow(...)]`, `@property` — trivial-to-add
/// boilerplate that an agent could sprinkle to perturb the seed).
fn is_skipped_kind(kind: &str) -> bool {
    is_comment_kind(kind) || is_attribute_kind(kind)
}

fn is_attribute_kind(kind: &str) -> bool {
    // Rust: `attribute_item` (outer `#[...]`), `inner_attribute_item`
    // (inner `#![...]`). JS/TS/Python: `decorator`. Java: `annotation`,
    // `marker_annotation`. Catch all of them — collateral on other
    // grammars would only ever remove attribute-shaped boilerplate.
    kind == "attribute_item"
        || kind == "inner_attribute_item"
        || kind == "attribute"
        || kind == "decorator"
        || kind == "annotation"
        || kind == "marker_annotation"
}

fn is_macro_invocation_kind(kind: &str) -> bool {
    // Rust's `macro_invocation` is the canonical case. Don't recurse into
    // the token tree — its contents are unconstrained user text and
    // sprinkling arguments into `dbg!()` / `println!()` shouldn't move
    // the score. The macro_invocation itself still emits one Expression
    // feature, so the call isn't free.
    kind == "macro_invocation" || kind == "macro_rule"
}

/// Reorder `nodes` so groups (one group = one top-level item) appear in
/// signature-sorted order, keeping in-group order intact. Nodes outside
/// any top-level item (the root, sentinel `u32::MAX`) stay at the front.
/// Closes the function-reorder lever: swapping two `fn` items changes
/// `row_raw` but not the per-group structural signature.
fn sort_groups_by_signature(nodes: Vec<AstNodeFeature>) -> Vec<AstNodeFeature> {
    use std::collections::HashMap;
    let mut groups: HashMap<u32, Vec<AstNodeFeature>> = HashMap::new();
    let mut insertion_order: Vec<u32> = Vec::new();
    for node in nodes {
        let key = node.top_level_idx;
        if !groups.contains_key(&key) {
            insertion_order.push(key);
        }
        groups.entry(key).or_default().push(node);
    }
    let mut signed: Vec<(u64, Vec<AstNodeFeature>)> = insertion_order
        .into_iter()
        .map(|key| {
            let group = groups.remove(&key).unwrap_or_default();
            // Sentinel `u32::MAX` = nodes outside any top-level item (the
            // root). Keep those at the front with signature `0`; real
            // items get a hash-derived signature clamped to be > 0.
            if key == u32::MAX {
                return (0, group);
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for node in &group {
                node.kind_weight.hash(&mut hasher);
                node.role_band.hash(&mut hasher);
                node.depth.hash(&mut hasher);
            }
            (hasher.finish().max(1), group)
        })
        .collect();
    signed.sort_by_key(|(sig, _)| *sig);
    signed.into_iter().flat_map(|(_, group)| group).collect()
}

fn hash_features(nodes: &[AstNodeFeature], _rows: &[RowMetrics]) -> u64 {
    // Hash only the structural triple `(kind_weight, role_band, depth)` per
    // node. Row-level metrics (`max_depth`, `control_flow_count` collapsed
    // by raw row number) are positional artefacts — comments push rows
    // down, blank lines change row counts, and the *same* AST then maps to
    // a different `rows: Vec` length. Including them in the hash would
    // re-introduce the comment / whitespace leak we're closing. The hash
    // must depend only on what the encoders actually project.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for node in nodes {
        node.kind_weight.hash(&mut hasher);
        node.role_band.hash(&mut hasher);
        node.depth.hash(&mut hasher);
    }
    hasher.finish()
}
