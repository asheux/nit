//! Language-detection shim for the structural encoder.
//!
//! The canonical implementation lives in [`crate::seed::encoders::lang`].
//! This module re-exports the pieces `StructuralEncoder` needs and adds
//! thin diagnostic helpers (`current_grammar_label`, `detect_label`) so
//! per-encoder debugging code doesn't have to reach across modules.

use std::path::Path;

#[allow(unused_imports)]
pub(super) use crate::seed::encoders::lang::seed_parse;
#[allow(unused_imports)]
pub(super) use crate::seed::encoders::lang::SeedLanguage;

/// Returns a stable lowercase identifier for the language detected at
/// `file_path`, or `"unknown"` if no grammar matches the extension.
/// Used by diagnostic logs that flag files the encoder couldn't parse.
#[allow(dead_code)]
pub(super) fn detect_label(file_path: Option<&Path>) -> &'static str {
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

/// Returns `true` when `file_path`'s extension maps to a grammar the
/// structural encoder can actually consume.
#[allow(dead_code)]
pub(super) fn is_supported(file_path: Option<&Path>) -> bool {
    detect_label(file_path) != "unknown"
}
