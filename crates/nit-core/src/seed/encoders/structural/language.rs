//! Language-detection shim for the structural encoder.
//!
//! Delegates to [`crate::languages`] (the single source of truth for
//! label / extension / filename mapping) and re-exports the pieces
//! `StructuralEncoder` needs, so per-encoder debug code doesn't have to
//! reach across modules.

use std::path::Path;

#[allow(unused_imports)]
pub(super) use crate::seed::encoders::lang::seed_parse;
#[allow(unused_imports)]
pub(super) use crate::seed::encoders::lang::SeedLanguage;

/// Canonical lowercase identifier for the grammar at `file_path`, or
/// `"unknown"` when no registered language matches.
#[allow(dead_code)]
pub(super) fn detect_label(file_path: Option<&Path>) -> &'static str {
    file_path
        .and_then(crate::languages::detect_by_path)
        .map(|info| info.label)
        .unwrap_or("unknown")
}

/// True when the structural encoder can consume the file's grammar.
#[allow(dead_code)]
pub(super) fn is_supported(file_path: Option<&Path>) -> bool {
    file_path
        .and_then(crate::languages::detect_by_path)
        .is_some()
}
