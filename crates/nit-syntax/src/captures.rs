//! Capture name to highlight group mapping and tree-sitter config builders.
//!
//! The [`CAPTURES`] table provides a single source of truth for the mapping
//! between tree-sitter capture names (e.g. `"keyword"`, `"string"`) and
//! the [`HighlightGroup`] variants consumed by the renderer.
//!
//! Three builder / accessor layers sit on top of that table:
//!
//! 1. [`capture_names`] / [`capture_group`] — low-level index accessors.
//! 2. [`build_highlight_configs`] — wraps `tree-sitter-highlight`.
//! 3. [`build_query_configs`] — wraps the raw `QueryCursor` path.
//!
//! [`CaptureCategory`] classifies groups into semantic families for
//! category-level operations (e.g. toggling all keywords in a theme).

use std::collections::HashMap;
use std::fmt;

use tracing::debug;
use tree_sitter::Query;
use tree_sitter_highlight::HighlightConfiguration;

use crate::highlight::HighlightGroup;
use crate::registry::{LanguageId, LanguageRegistry};

// ── Capture table ──────────────────────────────────────────────────────────

/// Unified mapping from tree-sitter capture names to [`HighlightGroup`]s.
///
/// **Order matters**: the index into this table is the highlight-event
/// index returned by `tree-sitter-highlight`. New entries must be
/// appended to preserve backward compatibility.
const CAPTURES: &[(&str, HighlightGroup)] = &[
    // — Annotations —
    ("comment", HighlightGroup::Comment),
    ("comment.documentation", HighlightGroup::DocComment),
    // — Literal values —
    ("string", HighlightGroup::String),
    ("string.special", HighlightGroup::String),
    ("character", HighlightGroup::Char),
    ("number", HighlightGroup::Number),
    ("boolean", HighlightGroup::Boolean),
    // — Language keywords —
    ("keyword", HighlightGroup::Keyword),
    ("keyword.control", HighlightGroup::KeywordControl),
    ("keyword.operator", HighlightGroup::KeywordOperator),
    // — Type system —
    ("type", HighlightGroup::Type),
    ("type.builtin", HighlightGroup::TypeBuiltin),
    // — Callables —
    ("function", HighlightGroup::Function),
    ("method", HighlightGroup::Method),
    ("macro", HighlightGroup::Macro),
    // — Declarations —
    ("attribute", HighlightGroup::Attribute),
    ("namespace", HighlightGroup::Namespace),
    // — Values —
    ("variable", HighlightGroup::Variable),
    ("parameter", HighlightGroup::Parameter),
    ("property", HighlightGroup::Property),
    ("constant", HighlightGroup::Constant),
    // — Operators and punctuation —
    ("operator", HighlightGroup::Operator),
    ("punctuation", HighlightGroup::Punctuation),
    // — Markup —
    ("tag", HighlightGroup::Tag),
    ("heading", HighlightGroup::Heading),
    ("emphasis", HighlightGroup::Emphasis),
    ("link", HighlightGroup::Link),
    // — Diagnostics —
    ("error", HighlightGroup::Error),
    ("warning", HighlightGroup::Warning),
    // — Aliases (dotted capture names that map to parent groups) —
    ("constant.builtin", HighlightGroup::Number),
    ("function.macro", HighlightGroup::Macro),
    ("function.method", HighlightGroup::Method),
    ("variable.parameter", HighlightGroup::Parameter),
    ("variable.builtin", HighlightGroup::Variable),
    ("punctuation.bracket", HighlightGroup::Punctuation),
    ("punctuation.delimiter", HighlightGroup::Punctuation),
    ("constructor", HighlightGroup::Type),
    ("label", HighlightGroup::KeywordControl),
    ("escape", HighlightGroup::String),
];

// ── Capture accessors ──────────────────────────────────────────────────────

/// Collect all capture names for [`HighlightConfiguration::configure`].
pub(crate) fn capture_names() -> Vec<&'static str> {
    CAPTURES
        .iter()
        .map(|(capture_name, _)| *capture_name)
        .collect()
}

/// Resolve a highlight-event index to its [`HighlightGroup`], falling
/// back to [`HighlightGroup::Normal`] for unknown indices.
pub(crate) fn capture_group(event_index: usize) -> HighlightGroup {
    CAPTURES
        .get(event_index)
        .map(|(_, highlight_group)| *highlight_group)
        .unwrap_or(HighlightGroup::Normal)
}

/// Total number of entries in the unified capture table.
///
/// Useful for pre-allocating buffers that need one slot per capture.
pub const fn capture_entry_count() -> usize {
    CAPTURES.len()
}

// ── Capture categories ────────────────────────────────────────────────────

/// Semantic family that groups related [`HighlightGroup`] variants.
///
/// Each entry in the [`CAPTURES`] table belongs to exactly one category.
/// This enables category-level operations such as toggling all keyword
/// highlights at once in a theme editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureCategory {
    /// Comments and documentation annotations.
    Annotation,
    /// String, character, number, and boolean literals.
    Literal,
    /// Control-flow and operator keywords.
    Keyword,
    /// Type names and built-in types.
    TypeSystem,
    /// Functions, methods, and macros.
    Callable,
    /// Attributes, namespaces, and declaration modifiers.
    Declaration,
    /// Variables, parameters, properties, and constants.
    Value,
    /// Symbolic operators and punctuation tokens.
    Operator,
    /// HTML/XML tags, headings, emphasis, and links.
    Markup,
    /// Error and warning diagnostic nodes.
    Diagnostic,
}

/// Number of distinct [`CaptureCategory`] variants.
pub const CATEGORY_COUNT: usize = 10;

// ── CaptureCategory Display ───────────────────────────────────────────────

impl fmt::Display for CaptureCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ── CaptureCategory classification ────────────────────────────────────────

impl CaptureCategory {
    /// Classify a [`HighlightGroup`] into its semantic category.
    ///
    /// Uses a glob import to keep the match arms concise. Groups that
    /// represent diff markers or the default `Normal` variant are
    /// classified as [`Value`](Self::Value).
    pub fn of_group(source_group: HighlightGroup) -> Self {
        use HighlightGroup::*;
        match source_group {
            Comment | DocComment => Self::Annotation,
            String | Char | Number | Boolean => Self::Literal,
            Keyword | KeywordControl | KeywordOperator => Self::Keyword,
            Type | TypeBuiltin => Self::TypeSystem,
            Function | Method | Macro => Self::Callable,
            Attribute | Namespace => Self::Declaration,
            Variable | Parameter | Property | Constant => Self::Value,
            Operator | Punctuation => Self::Operator,
            Tag | Heading | Emphasis | Link => Self::Markup,
            Error | Warning => Self::Diagnostic,
            _ => Self::Value,
        }
    }
}

// ── CaptureCategory queries ──────────────────────────────────────────────

impl CaptureCategory {
    /// Human-readable label for this category.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Annotation => "annotation",
            Self::Literal => "literal",
            Self::Keyword => "keyword",
            Self::TypeSystem => "type-system",
            Self::Callable => "callable",
            Self::Declaration => "declaration",
            Self::Value => "value",
            Self::Operator => "operator",
            Self::Markup => "markup",
            Self::Diagnostic => "diagnostic",
        }
    }

    /// Returns `true` when the category covers literal-value tokens
    /// (strings, characters, numbers, and booleans).
    pub fn is_literal(self) -> bool {
        matches!(self, Self::Literal)
    }
}

// ── Query config ───────────────────────────────────────────────────────────

/// Pre-compiled query paired with a per-capture highlight group vector
/// for a single language. Used by the raw `QueryCursor` path.
pub(crate) struct QueryConfig {
    /// Compiled tree-sitter query (highlights only, no injections).
    pub query: Query,
    /// One entry per capture in `query`; `None` for captures that have
    /// no matching [`HighlightGroup`].
    pub capture_groups: Vec<Option<HighlightGroup>>,
}

// ── QueryConfig accessors ─────────────────────────────────────────────────

impl QueryConfig {
    /// Resolve capture at `capture_idx` to a highlight group, defaulting
    /// to [`HighlightGroup::Normal`] when the index is out of range or
    /// the capture is unmapped.
    pub fn group_for_index(&self, capture_idx: usize) -> HighlightGroup {
        self.capture_groups
            .get(capture_idx)
            .copied()
            .flatten()
            .unwrap_or(HighlightGroup::Normal)
    }
}

// ── Highlight-configuration builder ────────────────────────────────────────

/// Build [`HighlightConfiguration`]s for every supported language.
///
/// Each configuration receives the unified capture name list so that
/// highlight-event indices align with the [`CAPTURES`] table.
pub(crate) fn build_highlight_configs() -> HashMap<LanguageId, HighlightConfiguration> {
    let ordered_capture_names = capture_names();
    let language_count: usize = LanguageId::ALL.len();
    let mut result_map: HashMap<LanguageId, HighlightConfiguration> =
        HashMap::with_capacity(language_count);

    for target_lang in LanguageId::ALL {
        let Some(ts_grammar) = LanguageRegistry::tree_sitter_language(target_lang) else {
            continue;
        };

        let highlight_source = LanguageRegistry::highlights_query(target_lang).unwrap_or("");
        let injection_source = LanguageRegistry::injections_query(target_lang);

        let Some(mut highlight_cfg) =
            try_build_config(ts_grammar, highlight_source, injection_source)
        else {
            debug!("highlight config for {target_lang:?} failed (with and without injections)");
            continue;
        };

        highlight_cfg.configure(&ordered_capture_names);
        result_map.insert(target_lang, highlight_cfg);
    }

    result_map
}

/// Attempt to create a [`HighlightConfiguration`], falling back to an
/// injection-free config if the injections query fails to parse.
fn try_build_config(
    grammar: tree_sitter::Language,
    highlight_source: &str,
    injection_source: &str,
) -> Option<HighlightConfiguration> {
    match HighlightConfiguration::new(grammar, highlight_source, injection_source, "") {
        Ok(built_config) => Some(built_config),
        Err(injection_error) => {
            debug!("injections failed ({injection_error}), retrying without");
            HighlightConfiguration::new(grammar, highlight_source, "", "").ok()
        }
    }
}

// ── Query-config builder ───────────────────────────────────────────────────

/// Build [`QueryConfig`]s for every supported language (used by the
/// raw `QueryCursor` path for incremental and viewport highlighting).
pub(crate) fn build_query_configs() -> HashMap<LanguageId, QueryConfig> {
    let group_table: HashMap<&str, HighlightGroup> = CAPTURES
        .iter()
        .map(|&(capture_label, mapped_group)| (capture_label, mapped_group))
        .collect();

    let total_languages: usize = LanguageId::ALL.len();
    let mut result_map: HashMap<LanguageId, QueryConfig> = HashMap::with_capacity(total_languages);

    for target_lang in LanguageId::ALL {
        let Some(ts_grammar) = LanguageRegistry::tree_sitter_language(target_lang) else {
            continue;
        };
        let Some(highlight_source) = LanguageRegistry::highlights_query(target_lang) else {
            continue;
        };
        let Ok(compiled_query) = Query::new(ts_grammar, highlight_source) else {
            continue;
        };

        let resolved_groups = resolve_capture_groups(&compiled_query, &group_table);

        result_map.insert(
            target_lang,
            QueryConfig {
                query: compiled_query,
                capture_groups: resolved_groups,
            },
        );
    }

    result_map
}

// ── Capture-group resolution ─────────────────────────────────────────────

/// Map each capture declared in a compiled query to its
/// [`HighlightGroup`], falling back to the dotless parent of dotted
/// capture names (e.g. `"variable.parameter"` → `"variable"`).
fn resolve_capture_groups(
    compiled_query: &Query,
    group_table: &HashMap<&str, HighlightGroup>,
) -> Vec<Option<HighlightGroup>> {
    compiled_query
        .capture_names()
        .iter()
        .map(|capture_name| lookup_with_parent_fallback(capture_name, group_table))
        .collect()
}

/// Look up a single capture name in the group table, trying the full
/// dotted name first and falling back to the root segment before the
/// first dot (e.g. `"function.method"` → `"function"`).
fn lookup_with_parent_fallback(
    full_capture_name: &str,
    group_table: &HashMap<&str, HighlightGroup>,
) -> Option<HighlightGroup> {
    if let Some(&direct_hit) = group_table.get(full_capture_name) {
        return Some(direct_hit);
    }
    let parent_prefix = full_capture_name.split('.').next()?;
    group_table.get(parent_prefix).copied()
}
