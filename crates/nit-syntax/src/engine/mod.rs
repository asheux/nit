//! Syntax engine trait + shared request/config DTOs.

use std::path::Path;

use nit_core::BufferEdit;

use crate::highlight::{EngineKind, HighlightSnapshot};
use crate::language::{LanguageId, LanguageRegistry};

mod manager;
mod plain;
pub(crate) mod tree_sitter;

pub use manager::SyntaxManager;
pub use plain::PlainTextEngine;

#[derive(Clone, Debug)]
pub struct SyntaxConfig {
    pub enabled: bool,
    pub engine: EngineKind,
    pub debounce_ms: u64,
    pub max_file_bytes: usize,
    pub max_spans_per_line: usize,
}

impl Default for SyntaxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: EngineKind::TreeSitter,
            debounce_ms: 50,
            max_file_bytes: 2_000_000,
            max_spans_per_line: 256,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViewportRange {
    pub first_line: usize,
    pub last_line: usize,
    pub total_lines: usize,
}

/// Sentinel buffer id for grammar prewarm jobs (no real buffer to write back to).
pub(crate) const PREWARM_BUFFER_ID: usize = usize::MAX;

#[derive(Clone, Debug)]
pub struct HighlightRequest {
    pub buffer_id: usize,
    pub version: u64,
    pub language: LanguageId,
    pub text: String,
    pub edits: Vec<BufferEdit>,
    pub full_reparse: bool,
    pub max_spans_per_line: usize,
    pub viewport: Option<ViewportRange>,
}

impl HighlightRequest {
    pub(crate) fn prewarm(language: LanguageId) -> Self {
        Self {
            buffer_id: PREWARM_BUFFER_ID,
            version: 0,
            language,
            text: String::new(),
            edits: Vec::new(),
            full_reparse: true,
            max_spans_per_line: 0,
            viewport: None,
        }
    }
}

pub trait SyntaxEngine {
    fn detect_language(
        &self,
        path: Option<&Path>,
        first_line: Option<&str>,
        override_lang: Option<LanguageId>,
    ) -> LanguageId {
        LanguageRegistry::detect(path, first_line, override_lang)
    }

    fn schedule_rehighlight(&mut self, request: HighlightRequest);

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot>;
}
