//! Language-detection helpers exposed at the `seed::encoders` root.
//!
//! Wraps [`crate::seed::encoders::lang`] so callers outside the encoder
//! tree can ask "what grammar does this path use?" without depending on
//! the internal `lang::SeedLanguage` enum. Useful for diagnostic surfaces
//! (genome reports, snapshot debug output) that want a stable label
//! rather than a strongly-typed enum.

use std::path::Path;

#[allow(unused_imports)]
pub(crate) use super::lang::{seed_parse, SeedLanguage};

/// Lowercase identifier for the grammar detected at `file_path`, or
/// `"unknown"` if no parser maps to the extension.
#[allow(dead_code)]
pub(crate) fn language_label(file_path: Option<&Path>) -> &'static str {
    match file_path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
    {
        Some(ext) => match ext.to_lowercase().as_str() {
            "rs" => "rust",
            "py" => "python",
            "js" | "mjs" | "cjs" | "jsx" => "javascript",
            "ts" | "tsx" => "typescript",
            "md" | "markdown" => "markdown",
            "html" | "htm" => "html",
            "css" | "scss" | "sass" => "css",
            "json" | "jsonc" => "json",
            "toml" => "toml",
            "yml" | "yaml" => "yaml",
            "sh" | "bash" | "zsh" | "fish" => "bash",
            _ => "unknown",
        },
        None => "unknown",
    }
}

/// Returns `true` when the file's extension maps to a grammar the AST
/// encoders can actually use.
#[allow(dead_code)]
pub(crate) fn is_supported_language(file_path: Option<&Path>) -> bool {
    language_label(file_path) != "unknown"
}
