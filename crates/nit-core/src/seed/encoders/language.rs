//! Public-facing language helpers for the seed encoders.
//!
//! Both helpers delegate to [`crate::languages`] — the single source of
//! truth for label / extension / filename mapping — so adding a language
//! never requires touching this file. Callers outside the encoder tree
//! use these instead of importing the internal `SeedLanguage` enum, so
//! diagnostic surfaces (genome reports, snapshot debug output) get a
//! stable string label rather than an opaque type.

use std::path::Path;

#[allow(unused_imports)]
pub(crate) use super::lang::{seed_parse, SeedLanguage};

/// Canonical lowercase identifier for the grammar at `file_path`, or
/// `"unknown"` if no registered language matches.
#[allow(dead_code)]
pub(crate) fn language_label(file_path: Option<&Path>) -> &'static str {
    file_path
        .and_then(crate::languages::detect_by_path)
        .map(|info| info.label)
        .unwrap_or("unknown")
}

/// True when the file resolves to a registered language. Drives the
/// encoder gate that skips files no grammar can parse.
#[allow(dead_code)]
pub(crate) fn is_supported_language(file_path: Option<&Path>) -> bool {
    file_path
        .and_then(crate::languages::detect_by_path)
        .is_some()
}
