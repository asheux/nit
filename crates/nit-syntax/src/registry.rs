//! Language identification and tree-sitter grammar registry.

use std::fmt;
use std::path::Path;

// ── Language identifier ────────────────────────────────────────────────────

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

/// Does not include `PlainText` (no grammar).
impl LanguageId {
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

// ── Registry ───────────────────────────────────────────────────────────────

pub struct LanguageRegistry;

// ── Detection ──────────────────────────────────────────────────────────────

impl LanguageRegistry {
    /// Priority: override > shebang > path > `PlainText`.
    pub fn detect(
        file_path: Option<&Path>,
        first_line: Option<&str>,
        explicit_override: Option<LanguageId>,
    ) -> LanguageId {
        if let Some(language) = explicit_override {
            return language;
        }
        if let Some(language) = first_line.and_then(detect_shebang) {
            return language;
        }
        file_path
            .map(detect_from_path)
            .unwrap_or(LanguageId::PlainText)
    }
}

// ── Grammar and query lookup ───────────────────────────────────────────────

impl LanguageRegistry {
    pub fn tree_sitter_language(language_id: LanguageId) -> Option<tree_sitter::Language> {
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

    /// Return the SCM highlights query source for a known language.
    pub fn highlights_query(language_id: LanguageId) -> Option<&'static str> {
        match language_id {
            LanguageId::Rust => Some(include_str!("../queries/rust/highlights.scm")),
            LanguageId::Python => Some(tree_sitter_python::HIGHLIGHT_QUERY),
            LanguageId::JavaScript => Some(tree_sitter_javascript::HIGHLIGHT_QUERY),
            LanguageId::TypeScript => Some(tree_sitter_typescript::HIGHLIGHT_QUERY),
            LanguageId::Markdown => Some(include_str!("../queries/markdown/highlights.scm")),
            LanguageId::Html => Some(tree_sitter_html::HIGHLIGHT_QUERY),
            LanguageId::Css => Some(tree_sitter_css::HIGHLIGHTS_QUERY),
            LanguageId::Json => Some(tree_sitter_json::HIGHLIGHT_QUERY),
            LanguageId::Toml => Some(tree_sitter_toml::HIGHLIGHT_QUERY),
            LanguageId::Yaml => Some(include_str!("../queries/yaml/highlights.scm")),
            LanguageId::Bash => Some(tree_sitter_bash::HIGHLIGHT_QUERY),
            LanguageId::PlainText => None,
        }
    }

    /// Return the SCM injections query (for embedded language blocks).
    pub fn injections_query(language_id: LanguageId) -> &'static str {
        match language_id {
            LanguageId::Markdown => include_str!("../queries/markdown/injections.scm"),
            LanguageId::Html => include_str!("../queries/html/injections.scm"),
            _ => "",
        }
    }

    /// Resolve an injection language name (from a tree-sitter grammar)
    /// back to a [`LanguageId`].
    pub fn from_injection_name(injection_name: &str) -> Option<LanguageId> {
        let token = injection_name
            .split(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '_')
            .next()
            .unwrap_or(injection_name);

        match token.to_lowercase().as_str() {
            "rust" => Some(LanguageId::Rust),
            "python" => Some(LanguageId::Python),
            "javascript" | "js" => Some(LanguageId::JavaScript),
            "typescript" | "ts" | "tsx" => Some(LanguageId::TypeScript),
            "markdown" | "md" => Some(LanguageId::Markdown),
            "html" => Some(LanguageId::Html),
            "css" => Some(LanguageId::Css),
            "json" => Some(LanguageId::Json),
            "toml" => Some(LanguageId::Toml),
            "yaml" | "yml" => Some(LanguageId::Yaml),
            "bash" | "sh" => Some(LanguageId::Bash),
            _ => None,
        }
    }
}

// ── Path-based detection ───────────────────────────────────────────────────

/// Match a file path against known filenames and extensions.
fn detect_from_path(file_path: &Path) -> LanguageId {
    // Check well-known filenames first (e.g. `Cargo.toml`, `Makefile`).
    if let Some(filename) = file_path.file_name().and_then(|os| os.to_str()) {
        match filename.to_lowercase().as_str() {
            "cargo.toml" => return LanguageId::Toml,
            "makefile" => return LanguageId::Bash,
            _ => {}
        }
    }

    // Fall back to extension matching.
    let extension = match file_path.extension().and_then(|os| os.to_str()) {
        Some(ext) => ext.to_lowercase(),
        None => return LanguageId::PlainText,
    };

    match extension.as_str() {
        "rs" => LanguageId::Rust,
        "py" => LanguageId::Python,
        "js" | "mjs" | "cjs" | "jsx" => LanguageId::JavaScript,
        "ts" | "tsx" => LanguageId::TypeScript,
        "md" | "markdown" => LanguageId::Markdown,
        "html" | "htm" => LanguageId::Html,
        "css" | "scss" | "sass" => LanguageId::Css,
        "json" | "jsonc" => LanguageId::Json,
        "toml" => LanguageId::Toml,
        "yml" | "yaml" => LanguageId::Yaml,
        "sh" | "bash" | "zsh" | "fish" => LanguageId::Bash,
        _ => LanguageId::PlainText,
    }
}

// ── Shebang detection ──────────────────────────────────────────────────────

/// Inspect the first line of a file for a `#!` shebang and map the
/// interpreter name to a [`LanguageId`].
fn detect_shebang(first_line: &str) -> Option<LanguageId> {
    let shebang_line = first_line.trim();
    if !shebang_line.starts_with("#!") {
        return None;
    }

    // Extract the interpreter name from the shebang path.
    // Handles both `#!/usr/bin/bash` and `#!/usr/bin/env bash`.
    let after_hash = &shebang_line[2..];
    let interpreter = after_hash
        .rsplit('/')
        .next()
        .unwrap_or(after_hash)
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_lowercase();

    match interpreter.as_str() {
        "bash" | "sh" | "zsh" => Some(LanguageId::Bash),
        "python" | "python3" => Some(LanguageId::Python),
        "node" | "deno" => Some(LanguageId::JavaScript),
        _ => None,
    }
}
