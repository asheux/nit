//! Capture name → highlight group mapping and tree-sitter config builders.

use std::collections::HashMap;
use std::fmt;

use tracing::debug;
use tree_sitter::Query;
use tree_sitter_highlight::HighlightConfiguration;

use crate::highlight::HighlightGroup;
use crate::registry::{LanguageId, LanguageRegistry};

// ── Capture table ──────────────────────────────────────────────────────────

/// Index order matches highlight-event IDs from `tree-sitter-highlight`;
/// new entries must be appended.
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

pub(crate) fn capture_names() -> Vec<&'static str> {
    CAPTURES.iter().map(|(name, _)| *name).collect()
}

pub(crate) fn capture_group(event_index: usize) -> HighlightGroup {
    CAPTURES
        .get(event_index)
        .map(|(_, group)| *group)
        .unwrap_or(HighlightGroup::Normal)
}

pub const fn capture_entry_count() -> usize {
    CAPTURES.len()
}

// ── Capture categories ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureCategory {
    Annotation,
    Literal,
    Keyword,
    TypeSystem,
    Callable,
    Declaration,
    Value,
    Operator,
    Markup,
    Diagnostic,
}

pub const CATEGORY_COUNT: usize = CaptureCategory::ALL.len();

impl CaptureCategory {
    const ALL: [Self; 10] = [
        Self::Annotation,
        Self::Literal,
        Self::Keyword,
        Self::TypeSystem,
        Self::Callable,
        Self::Declaration,
        Self::Value,
        Self::Operator,
        Self::Markup,
        Self::Diagnostic,
    ];

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

    pub fn of_group(group: HighlightGroup) -> Self {
        use HighlightGroup::*;
        match group {
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

    pub fn is_literal(self) -> bool {
        matches!(self, Self::Literal)
    }
}

impl fmt::Display for CaptureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── QueryConfig ───────────────────────────────────────────────────────────

pub(crate) struct QueryConfig {
    pub query: Query,
    pub highlight_groups: Vec<Option<HighlightGroup>>,
}

impl QueryConfig {
    pub fn highlight_for_index(&self, capture_idx: usize) -> HighlightGroup {
        self.highlight_groups
            .get(capture_idx)
            .copied()
            .flatten()
            .unwrap_or(HighlightGroup::Normal)
    }
}

// ── Language iteration helper ──────────────────────────────────────────────

/// Yield `(LanguageId, grammar, highlights_query)` for every language with
/// a bound tree-sitter grammar, skipping languages whose grammar or query
/// is unavailable.
fn for_each_grammar(mut f: impl FnMut(LanguageId, tree_sitter::Language, &'static str)) {
    for lang in LanguageId::ALL {
        let Some(grammar) = LanguageRegistry::tree_sitter_language(lang) else {
            continue;
        };
        let Some(highlights) = LanguageRegistry::highlights_query(lang) else {
            continue;
        };
        f(lang, grammar, highlights);
    }
}

// ── Highlight-configuration builder ────────────────────────────────────────

pub(crate) fn build_highlight_configs() -> HashMap<LanguageId, HighlightConfiguration> {
    let names = capture_names();
    let mut configs: HashMap<LanguageId, HighlightConfiguration> =
        HashMap::with_capacity(LanguageId::ALL.len());

    for_each_grammar(|lang, grammar, highlights| {
        let injections = LanguageRegistry::injections_query(lang);
        let Some(mut cfg) = try_build_config(grammar, highlights, injections) else {
            debug!("highlight config for {lang:?} failed (with and without injections)");
            return;
        };
        cfg.configure(&names);
        configs.insert(lang, cfg);
    });

    configs
}

fn try_build_config(
    grammar: tree_sitter::Language,
    highlights: &str,
    injections: &str,
) -> Option<HighlightConfiguration> {
    match HighlightConfiguration::new(grammar, highlights, injections, "") {
        Ok(cfg) => Some(cfg),
        Err(err) => {
            debug!("injections failed ({err}), retrying without");
            HighlightConfiguration::new(grammar, highlights, "", "").ok()
        }
    }
}

// ── Query-config builder ───────────────────────────────────────────────────

pub(crate) fn build_query_configs() -> HashMap<LanguageId, QueryConfig> {
    let groups: HashMap<&str, HighlightGroup> = CAPTURES.iter().copied().collect();
    let mut configs: HashMap<LanguageId, QueryConfig> =
        HashMap::with_capacity(LanguageId::ALL.len());

    for_each_grammar(|lang, grammar, highlights| {
        let Ok(query) = Query::new(grammar, highlights) else {
            return;
        };
        let highlight_groups = resolve_highlight_groups(&query, &groups);
        configs.insert(
            lang,
            QueryConfig {
                query,
                highlight_groups,
            },
        );
    });

    configs
}

// ── Capture-group resolution ─────────────────────────────────────────────

fn resolve_highlight_groups(
    query: &Query,
    groups: &HashMap<&str, HighlightGroup>,
) -> Vec<Option<HighlightGroup>> {
    query
        .capture_names()
        .iter()
        .map(|name| lookup_with_parent_fallback(name, groups))
        .collect()
}

/// Falls back to the root segment of dotted names (e.g. `"function.method"` → `"function"`).
fn lookup_with_parent_fallback(
    name: &str,
    groups: &HashMap<&str, HighlightGroup>,
) -> Option<HighlightGroup> {
    if let Some(&group) = groups.get(name) {
        return Some(group);
    }
    let root = name.split('.').next()?;
    groups.get(root).copied()
}
