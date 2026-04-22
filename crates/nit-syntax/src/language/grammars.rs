//! Tree-sitter grammar and highlight-query lookups keyed by [`LanguageId`].

use super::id::LanguageId;

#[must_use]
pub(crate) fn tree_sitter_language(language_id: LanguageId) -> Option<tree_sitter::Language> {
    match language_id {
        LanguageId::Rust => Some(tree_sitter_rust::language()),
        LanguageId::Python => Some(tree_sitter_python::language()),
        LanguageId::JavaScript => Some(tree_sitter_javascript::language()),
        LanguageId::TypeScript => Some(tree_sitter_typescript::language_typescript()),
        LanguageId::Markdown => Some(tree_sitter_markdown_fork::language()),
        LanguageId::Html => Some(tree_sitter_html::language()),
        LanguageId::Css => Some(tree_sitter_css::language()),
        LanguageId::Json => Some(tree_sitter_json::language()),
        LanguageId::Toml => Some(tree_sitter_toml::language()),
        LanguageId::Yaml => Some(tree_sitter_yaml::language()),
        LanguageId::Bash => Some(tree_sitter_bash::language()),
        LanguageId::PlainText => None,
    }
}

#[must_use]
pub(crate) fn highlights_query(language_id: LanguageId) -> Option<&'static str> {
    match language_id {
        LanguageId::Rust => Some(include_str!("../../queries/rust/highlights.scm")),
        LanguageId::Python => Some(tree_sitter_python::HIGHLIGHT_QUERY),
        LanguageId::JavaScript => Some(tree_sitter_javascript::HIGHLIGHT_QUERY),
        LanguageId::TypeScript => Some(tree_sitter_typescript::HIGHLIGHT_QUERY),
        LanguageId::Markdown => Some(include_str!("../../queries/markdown/highlights.scm")),
        LanguageId::Html => Some(tree_sitter_html::HIGHLIGHT_QUERY),
        LanguageId::Css => Some(tree_sitter_css::HIGHLIGHTS_QUERY),
        LanguageId::Json => Some(tree_sitter_json::HIGHLIGHT_QUERY),
        LanguageId::Toml => Some(tree_sitter_toml::HIGHLIGHT_QUERY),
        LanguageId::Yaml => Some(include_str!("../../queries/yaml/highlights.scm")),
        LanguageId::Bash => Some(tree_sitter_bash::HIGHLIGHT_QUERY),
        LanguageId::PlainText => None,
    }
}

#[must_use]
pub(crate) fn injections_query(language_id: LanguageId) -> &'static str {
    match language_id {
        LanguageId::Markdown => include_str!("../../queries/markdown/injections.scm"),
        LanguageId::Html => include_str!("../../queries/html/injections.scm"),
        _ => "",
    }
}
