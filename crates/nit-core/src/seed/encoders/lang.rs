//! Tree-sitter language detection and parsing for the seed encoders.
//! `SeedLanguage` is duplicated here (rather than reused from `nit-syntax`) to
//! avoid a dependency cycle.

use std::path::Path;

use tree_sitter::{Parser, Tree};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SeedLanguage {
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
}

impl SeedLanguage {
    fn detect(file_path: Option<&Path>) -> Option<Self> {
        let path = file_path?;
        let ext = path.extension()?.to_str()?;
        match ext.to_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "js" | "mjs" | "cjs" | "jsx" => Some(Self::JavaScript),
            "ts" | "tsx" => Some(Self::TypeScript),
            "md" | "markdown" => Some(Self::Markdown),
            "html" | "htm" => Some(Self::Html),
            "css" | "scss" | "sass" => Some(Self::Css),
            "json" | "jsonc" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            "yml" | "yaml" => Some(Self::Yaml),
            "sh" | "bash" | "zsh" | "fish" => Some(Self::Bash),
            _ => None,
        }
    }

    fn ts_language(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::language(),
            Self::Python => tree_sitter_python::language(),
            Self::JavaScript => tree_sitter_javascript::language(),
            Self::TypeScript => tree_sitter_typescript::language_typescript(),
            Self::Markdown => tree_sitter_markdown_fork::language(),
            Self::Html => tree_sitter_html::language(),
            Self::Css => tree_sitter_css::language(),
            Self::Json => tree_sitter_json::language(),
            Self::Toml => tree_sitter_toml::language(),
            Self::Yaml => tree_sitter_yaml::language(),
            Self::Bash => tree_sitter_bash::language(),
        }
    }
}

pub(crate) fn seed_parse(text: &str, file_path: Option<&Path>) -> Option<(Tree, SeedLanguage)> {
    let lang = SeedLanguage::detect(file_path)?;
    let mut parser = Parser::new();
    parser.set_language(lang.ts_language()).ok()?;
    let tree = parser.parse(text, None)?;
    Some((tree, lang))
}
