//! `LanguageId` enum and its ordered list of grammar-backed variants.

use std::fmt;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LanguageId {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Markdown,
    Html,
    Css,
    Json,
    Toml,
    Yaml,
    Bash,
    PlainText,
}

impl LanguageId {
    /// Every language that ships a grammar (excludes [`Self::PlainText`]).
    pub const ALL: [LanguageId; 11] = [
        Self::Rust,
        Self::Python,
        Self::JavaScript,
        Self::TypeScript,
        Self::Markdown,
        Self::Html,
        Self::Css,
        Self::Json,
        Self::Toml,
        Self::Yaml,
        Self::Bash,
    ];
}

impl fmt::Display for LanguageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Markdown => "Markdown",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Yaml => "YAML",
            Self::Bash => "Bash",
            Self::PlainText => "Plain Text",
        })
    }
}
