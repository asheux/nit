//! Language identification and tree-sitter grammar registry.
//!
//! To add a new language: append a variant to [`LanguageId`] and
//! [`LanguageId::ALL`], extend the path / shebang matches in [`detect`],
//! and add grammar + highlight-query entries in [`grammars`]. If the language
//! can appear in Markdown/HTML fenced blocks, also extend
//! [`detect::from_injection_name`].

use std::path::Path;

mod detect;
mod grammars;
mod id;

pub use id::LanguageId;

pub struct LanguageRegistry;

impl LanguageRegistry {
    /// Precedence: explicit override → shebang → path → `PlainText`.
    #[must_use]
    pub fn detect(
        file_path: Option<&Path>,
        first_line: Option<&str>,
        explicit_override: Option<LanguageId>,
    ) -> LanguageId {
        detect::detect(file_path, first_line, explicit_override)
    }

    #[must_use]
    pub fn tree_sitter_language(language_id: LanguageId) -> Option<tree_sitter::Language> {
        grammars::tree_sitter_language(language_id)
    }

    #[must_use]
    pub fn highlights_query(language_id: LanguageId) -> Option<&'static str> {
        grammars::highlights_query(language_id)
    }

    #[must_use]
    pub fn injections_query(language_id: LanguageId) -> &'static str {
        grammars::injections_query(language_id)
    }

    #[must_use]
    pub fn from_injection_name(injection_name: &str) -> Option<LanguageId> {
        detect::from_injection_name(injection_name)
    }
}
