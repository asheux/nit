//! Language detection: explicit override → shebang → path → `PlainText`,
//! plus an injection-alias lookup used for Markdown/HTML fenced blocks.

use std::path::Path;

use super::id::LanguageId;

#[must_use]
pub(crate) fn detect(
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

fn detect_from_path(file_path: &Path) -> LanguageId {
    if let Some(filename) = file_path.file_name().and_then(|os| os.to_str()) {
        match filename.to_lowercase().as_str() {
            "cargo.toml" => return LanguageId::Toml,
            "makefile" => return LanguageId::Bash,
            _ => {}
        }
    }

    let Some(extension) = file_path.extension().and_then(|os| os.to_str()) else {
        return LanguageId::PlainText;
    };

    match extension.to_lowercase().as_str() {
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

fn detect_shebang(first_line: &str) -> Option<LanguageId> {
    let line = first_line.trim();
    let after_hash = line.strip_prefix("#!")?;

    // Use the last whitespace-separated word so `/usr/bin/env python3`
    // resolves to `python3` rather than `env`.
    let interpreter = after_hash
        .split_whitespace()
        .last()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_lowercase();

    match interpreter.as_str() {
        "bash" | "sh" | "zsh" => Some(LanguageId::Bash),
        "python" | "python3" => Some(LanguageId::Python),
        "node" | "deno" => Some(LanguageId::JavaScript),
        _ => None,
    }
}

/// Used for Markdown/HTML fenced blocks: accepts aliases like
/// `js`, `ts`, `md`, `yml`, `sh` that the path detector does not.
#[must_use]
pub(crate) fn from_injection_name(injection_name: &str) -> Option<LanguageId> {
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
