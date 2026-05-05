//! Capture-name lookup table mapping tree-sitter highlight events to
//! [`HighlightGroup`] variants.

use crate::highlight::HighlightGroup;

// Index order IS the highlight-event id from `tree-sitter-highlight`;
// new entries must be appended, never reordered.
pub(crate) const CAPTURES: &[(&str, HighlightGroup)] = &[
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
    // Out-of-range events fall back to Normal in release; assert in dev so a
    // future drop/reorder of CAPTURES surfaces here, not as silent miscoloring.
    debug_assert!(
        event_index < CAPTURES.len(),
        "tree-sitter event index {event_index} exceeds CAPTURES len {}",
        CAPTURES.len()
    );
    CAPTURES
        .get(event_index)
        .map(|(_, group)| *group)
        .unwrap_or(HighlightGroup::Normal)
}

#[must_use]
pub const fn capture_entry_count() -> usize {
    CAPTURES.len()
}
