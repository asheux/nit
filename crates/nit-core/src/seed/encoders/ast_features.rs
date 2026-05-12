//! Canonical AST features shared by every quality encoder.
//!
//! Encoders consume this projection — never raw source text — so comment
//! padding, identifier renames, whitespace reflow, and attribute stuffing
//! can't perturb the seed. Each `AstNodeFeature` carries the role band
//! (declaration / control-flow / etc.), a 0-255 kind weight, AST depth,
//! source row (for row-grouped metrics only), the depth-1 ancestor index
//! (for signature-sorted item ordering), and the control-flow ancestor
//! count (for cognitive complexity). Comments / attributes / macro
//! invocations are excluded — they're gaming levers an agent can sprinkle
//! without changing program behaviour.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::path::Path;

use nit_utils::hashing::stable_hash_bytes;
use tree_sitter::Tree;

use super::node_class::ast_node_class;

pub(crate) use super::lang::seed_parse;

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct AstFeatures {
    pub(crate) nodes: Vec<AstNodeFeature>,
    /// Number of distinct rows that contain at least one significant node.
    pub(crate) significant_rows: usize,
    /// Per-row aggregates — retained for invariance tests + future row-aware
    /// encoders; the live encoders index `nodes` directly because raw-row
    /// bucketing is sensitive to comment / blank-line shifts.
    pub(crate) rows: Vec<RowMetrics>,
    /// Stable hash of the `(kind_weight, role_band, depth, cf_depth)` tuple
    /// per node. Drives `apply_structural_noise`; replaces the byte hash that
    /// used to leak comment / identifier / whitespace changes into the seed.
    pub(crate) feature_hash: u64,
}

#[derive(Clone)]
pub(crate) struct AstNodeFeature {
    pub(crate) kind_weight: u8,
    pub(crate) role_band: RoleBand,
    pub(crate) depth: u8,
    pub(crate) row_raw: u32,
    /// Index of the depth-1 ancestor (the "top-level item" this node belongs
    /// to). Used to sort items by structural signature so swapping two `fn`
    /// items at the top level produces the same node sequence. `u32::MAX`
    /// for nodes at depth ≤ 1.
    pub(crate) top_level_idx: u32,
    /// Count of `RoleBand::ControlFlow` ancestors. Drives cognitive
    /// complexity: each control-flow node contributes `1 + cf_depth`,
    /// penalising nested ladders cyclomatic-complexity alone treats as
    /// linear branch count.
    pub(crate) cf_depth: u8,
}

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
    pub(super) max_depth: u8,
    pub(super) control_flow_count: u8,
}

/// Decision returned by `classify_kind_for_skip`. Bundles three formerly
/// separate predicates so the walker takes one classification pass per node
/// instead of three.
enum SkipDecision {
    /// Comments + attributes — drop the whole subtree.
    Skip,
    /// Macro invocations — emit the macro's own feature but don't recurse
    /// into the token tree (whose contents are unconstrained user text).
    CollapseMacro,
    Walk,
}

fn classify_kind_for_skip(kind: &str) -> SkipDecision {
    // Comments: `line_comment` / `block_comment` / `comment` / `doc_comment`.
    // Attributes: Rust `attribute_item` / `inner_attribute_item`; JS/TS/Py
    // `decorator`; Java `annotation` / `marker_annotation`.
    let is_comment = matches!(
        kind,
        "line_comment" | "block_comment" | "comment" | "doc_comment"
    );
    let is_attribute = matches!(
        kind,
        "attribute_item"
            | "inner_attribute_item"
            | "attribute"
            | "decorator"
            | "annotation"
            | "marker_annotation"
    );
    if is_comment || is_attribute {
        return SkipDecision::Skip;
    }
    if matches!(kind, "macro_invocation" | "macro_rule") {
        return SkipDecision::CollapseMacro;
    }
    SkipDecision::Walk
}

/// Build the canonical features for a file. Returns `None` only when
/// tree-sitter can't parse it (unknown extension, parser missing).
/// Encoders treat `None` as "uniform / degenerate output"; they do not
/// fall back to byte-level processing.
pub(crate) fn compute_ast_features(text: &str, file_path: Option<&Path>) -> Option<AstFeatures> {
    // Thread-local 1-entry cache: within a single `compute_genome_report`,
    // `count_significant_lines`, parsimony, four encoders, and the jitter
    // hash all hit this function with the same `(text, file_path)`. Seven+
    // invocations collapse to one parse. Across files the slot is replaced.
    let content_hash = stable_hash_bytes(text.as_bytes());
    let path_hash = file_path
        .map(|p| stable_hash_bytes(p.to_string_lossy().as_bytes()))
        .unwrap_or(0);
    let key = (content_hash, path_hash);
    if let Some(cached) = AST_FEATURES_CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, f)| f.clone())
    }) {
        return Some(cached);
    }
    let (tree, _lang) = seed_parse(text, file_path)?;
    let features = build_features(text, &tree);
    AST_FEATURES_CACHE.with(|c| {
        *c.borrow_mut() = Some((key, features.clone()));
    });
    Some(features)
}

thread_local! {
    static AST_FEATURES_CACHE: RefCell<Option<((u64, u64), AstFeatures)>> =
        const { RefCell::new(None) };
}

fn build_features(text: &str, tree: &Tree) -> AstFeatures {
    let mut nodes = Vec::new();
    let mut significant_rows_set = std::collections::BTreeSet::new();
    let mut row_max_depth: std::collections::HashMap<u32, u8> = std::collections::HashMap::new();
    let mut row_cf_count: std::collections::HashMap<u32, u8> = std::collections::HashMap::new();

    // Iterative DFS — recursion would stack-overflow on pathological trees.
    // Stack tuple: `(node, depth, top_level_idx, cf_depth)`.
    let mut stack: Vec<(tree_sitter::Node, u32, u32, u8)> =
        vec![(tree.root_node(), 0, u32::MAX, 0)];
    let mut next_top_level_idx: u32 = 0;
    while let Some((node, depth, parent_top_level, parent_cf_depth)) = stack.pop() {
        let kind = node.kind();
        let decision = classify_kind_for_skip(kind);
        if matches!(decision, SkipDecision::Skip) {
            continue;
        }
        let my_top_level_idx = if depth == 1 {
            let idx = next_top_level_idx;
            next_top_level_idx = next_top_level_idx.saturating_add(1);
            idx
        } else {
            parent_top_level
        };
        let role_band = classify_role(kind);
        let child_cf_depth = if role_band == RoleBand::ControlFlow {
            parent_cf_depth.saturating_add(1)
        } else {
            parent_cf_depth
        };
        if !matches!(decision, SkipDecision::CollapseMacro) {
            let mut walker = node.walk();
            for child in node.children(&mut walker) {
                stack.push((child, depth + 1, my_top_level_idx, child_cf_depth));
            }
        }
        if !node.is_named() {
            continue;
        }
        // Unrecognised by the six structural bands — could be a legitimate
        // kind we haven't catalogued or a gaming surface. We can't tell, so
        // we don't emit a feature. Real grammars get explicit support via
        // `classify_role`. Children are still walked so their meaningful
        // kinds aren't lost.
        if role_band == RoleBand::Other {
            continue;
        }
        let depth_u8 = depth.min(255) as u8;
        let row_u32 = node.start_position().row as u32;
        let kind_weight = ast_node_class(kind);
        nodes.push(AstNodeFeature {
            kind_weight,
            role_band,
            depth: depth_u8,
            row_raw: row_u32,
            top_level_idx: my_top_level_idx,
            cf_depth: parent_cf_depth,
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
    // 1. Within each top-level item, restore source order by `(row_raw,
    //    depth)`. The DFS pushes children in source order then pops in
    //    reverse, so a per-group source-order sort is required.
    // 2. Reorder groups by structural signature (stable hash of feature
    //    triples). Swapping two `fn` items changes `row_raw` but not the
    //    signature, so the post-sort sequence is reorder-invariant.
    nodes.sort_by(|a, b| (a.row_raw, a.depth).cmp(&(b.row_raw, b.depth)));
    let nodes = sort_groups_by_signature(nodes);

    // Collapse raw row indices to dense "significant row" positions so
    // metrics aren't shifted by blank-line / comment-only rows.
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
    let _ = text; // invariant proof: text is deliberately unused
    AstFeatures {
        nodes,
        significant_rows,
        rows,
        feature_hash,
    }
}

pub(crate) fn classify_role(kind: &str) -> RoleBand {
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

/// Reorder nodes so groups (one group = one top-level item) appear in
/// signature-sorted order; in-group order is preserved. Closes the
/// function-reorder lever — swapping two `fn` items changes `row_raw` but
/// not the per-group structural signature.
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
            // `u32::MAX` is the sentinel for nodes outside any top-level
            // item (the root); keep those at the front with signature 0.
            if key == u32::MAX {
                return (0, group);
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for node in &group {
                node.kind_weight.hash(&mut hasher);
                node.role_band.hash(&mut hasher);
                node.depth.hash(&mut hasher);
                node.cf_depth.hash(&mut hasher);
            }
            (hasher.finish().max(1), group)
        })
        .collect();
    signed.sort_by_key(|(sig, _)| *sig);
    signed.into_iter().flat_map(|(_, group)| group).collect()
}

fn hash_features(nodes: &[AstNodeFeature], _rows: &[RowMetrics]) -> u64 {
    // Hash only the structural triple — row-level aggregates are positional
    // artefacts (comments push rows down, blank lines shift row counts) and
    // including them would re-introduce the leak we're closing.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for node in nodes {
        node.kind_weight.hash(&mut hasher);
        node.role_band.hash(&mut hasher);
        node.depth.hash(&mut hasher);
        node.cf_depth.hash(&mut hasher);
    }
    hasher.finish()
}
