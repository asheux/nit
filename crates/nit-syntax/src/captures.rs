//! Capture name → highlight group mapping and tree-sitter config builders.

use std::collections::HashMap;
use std::fmt;

use tracing::debug;
use tree_sitter::Query;
use tree_sitter_highlight::HighlightConfiguration;

use crate::highlight::HighlightGroup;
use crate::registry::{LanguageId, LanguageRegistry};

// Index order IS the highlight-event id from `tree-sitter-highlight`;
// new entries must be appended, never reordered.
const CAPTURES: &[(&str, HighlightGroup)] = &[
    ("comment", HighlightGroup::Comment),
    ("comment.documentation", HighlightGroup::DocComment),
    ("string", HighlightGroup::String),
    ("string.special", HighlightGroup::String),
    ("character", HighlightGroup::Char),
    ("number", HighlightGroup::Number),
    ("boolean", HighlightGroup::Boolean),
    ("keyword", HighlightGroup::Keyword),
    ("keyword.control", HighlightGroup::KeywordControl),
    ("keyword.operator", HighlightGroup::KeywordOperator),
    ("type", HighlightGroup::Type),
    ("type.builtin", HighlightGroup::TypeBuiltin),
    ("function", HighlightGroup::Function),
    ("method", HighlightGroup::Method),
    ("macro", HighlightGroup::Macro),
    ("attribute", HighlightGroup::Attribute),
    ("namespace", HighlightGroup::Namespace),
    ("variable", HighlightGroup::Variable),
    ("parameter", HighlightGroup::Parameter),
    ("property", HighlightGroup::Property),
    ("constant", HighlightGroup::Constant),
    ("operator", HighlightGroup::Operator),
    ("punctuation", HighlightGroup::Punctuation),
    ("tag", HighlightGroup::Tag),
    ("heading", HighlightGroup::Heading),
    ("emphasis", HighlightGroup::Emphasis),
    ("link", HighlightGroup::Link),
    ("error", HighlightGroup::Error),
    ("warning", HighlightGroup::Warning),
    // Dotted aliases that fall back to a parent group.
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

pub(crate) fn capture_names() -> Vec<&'static str> {
    CAPTURES.iter().map(|(name, _)| *name).collect()
}

pub(crate) fn capture_group(event_index: usize) -> HighlightGroup {
    CAPTURES
        .get(event_index)
        .map(|(_, group)| *group)
        .unwrap_or(HighlightGroup::Normal)
}

#[must_use]
pub const fn capture_entry_count() -> usize {
    CAPTURES.len()
}

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

    #[must_use]
    pub fn of_group(group: HighlightGroup) -> Self {
        use HighlightGroup::*;
        match group {
            Comment | DocComment => Self::Annotation,
            String | Char | Number | Boolean => Self::Literal,
            Keyword | KeywordControl | KeywordOperator => Self::Keyword,
            Type | TypeBuiltin => Self::TypeSystem,
            Function | Method | Macro => Self::Callable,
            Attribute | Namespace => Self::Declaration,
            Normal | Variable | Parameter | Property | Constant | DiffAdd | DiffRemove => {
                Self::Value
            }
            Operator | Punctuation => Self::Operator,
            Tag | Heading | Emphasis | Link => Self::Markup,
            Error | Warning => Self::Diagnostic,
        }
    }

    #[must_use]
    pub const fn is_literal(self) -> bool {
        matches!(self, Self::Literal)
    }
}

impl fmt::Display for CaptureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub trait Categorizable {
    fn category(&self) -> CaptureCategory;

    fn belongs_to(&self, target: CaptureCategory) -> bool {
        self.category() == target
    }
}

impl Categorizable for HighlightGroup {
    fn category(&self) -> CaptureCategory {
        CaptureCategory::of_group(*self)
    }
}

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

fn grammar_entries() -> impl Iterator<Item = (LanguageId, tree_sitter::Language, &'static str)> {
    LanguageId::ALL.into_iter().filter_map(|lang| {
        let grammar = LanguageRegistry::tree_sitter_language(lang)?;
        let highlights = LanguageRegistry::highlights_query(lang)?;
        Some((lang, grammar, highlights))
    })
}

pub(crate) fn build_highlight_configs() -> HashMap<LanguageId, HighlightConfiguration> {
    let names = capture_names();
    let mut configs = HashMap::with_capacity(LanguageId::ALL.len());

    for (lang, grammar, highlights) in grammar_entries() {
        let injections = LanguageRegistry::injections_query(lang);
        let Some(mut cfg) = try_build_config(grammar, highlights, injections) else {
            debug!("highlight config for {lang:?} failed (with and without injections)");
            continue;
        };
        cfg.configure(&names);
        configs.insert(lang, cfg);
    }

    configs
}

// Injection queries can fail to compile for some grammars; retry without them
// so the language still loads with bare highlights.
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

pub(crate) fn build_query_configs() -> HashMap<LanguageId, QueryConfig> {
    let groups: HashMap<&str, HighlightGroup> = CAPTURES.iter().copied().collect();
    let mut configs = HashMap::with_capacity(LanguageId::ALL.len());

    for (lang, grammar, highlights) in grammar_entries() {
        let Ok(query) = Query::new(grammar, highlights) else {
            continue;
        };
        let highlight_groups = resolve_highlight_groups(&query, &groups);
        configs.insert(
            lang,
            QueryConfig {
                query,
                highlight_groups,
            },
        );
    }

    configs
}

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

// `"function.method"` falls back to `"function"` when no exact match exists.
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
