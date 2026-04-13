//! Syntax engine configuration, trait definitions, and implementations.

use std::collections::HashMap;
use std::path::Path;

use nit_core::BufferEdit;

use crate::highlight::{EngineKind, HighlightSnapshot, SyntaxStatus};
use crate::registry::{LanguageId, LanguageRegistry};
use crate::tree_sitter_engine::TreeSitterEngine;

// ── Configuration ─────────────────────────────────────────────────────────

/// Tuning knobs for the syntax highlighting subsystem.
#[derive(Clone, Debug)]
pub struct SyntaxConfig {
    /// Master switch — when `false`, all buffers fall back to plain text.
    pub enabled: bool,
    /// Which backend to use globally.
    pub engine: EngineKind,
    /// Minimum milliseconds between successive rehighlight requests.
    pub debounce_ms: u64,
    /// Files larger than this (in bytes) skip tree-sitter parsing.
    pub max_file_bytes: usize,
    /// Per-line cap on highlight segments to avoid render stalls.
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

// ── Viewport ──────────────────────────────────────────────────────────────

/// Visible line range sent with each highlight request so large files can
/// prioritise the on-screen region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewportRange {
    pub first_line: usize,
    pub last_line: usize,
    pub total_lines: usize,
}

// ── Highlight request ─────────────────────────────────────────────────────

/// A request to (re)highlight a single buffer.
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

// ── Engine trait ──────────────────────────────────────────────────────────

/// Common interface implemented by every highlighting backend.
pub trait SyntaxEngine {
    /// Determine the language for a buffer from its path, shebang line,
    /// or an explicit override.
    fn detect_language(
        &self,
        path: Option<&Path>,
        first_line: Option<&str>,
        override_lang: Option<LanguageId>,
    ) -> LanguageId {
        LanguageRegistry::detect(path, first_line, override_lang)
    }

    /// Submit a highlight request.
    fn schedule_rehighlight(&mut self, request: HighlightRequest);

    /// Poll for a completed snapshot. Returns `None` if not ready.
    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot>;
}

// ── Plain-text fallback engine ────────────────────────────────────────────

/// Trivial engine that produces empty highlight spans.
#[derive(Default)]
pub struct PlainTextEngine {
    snapshots: HashMap<usize, HighlightSnapshot>,
}

impl PlainTextEngine {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SyntaxEngine for PlainTextEngine {
    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        let snap = HighlightSnapshot::plain(
            request.buffer_id,
            request.version,
            request.language,
            EngineKind::Plain,
            SyntaxStatus::Ok(EngineKind::Plain),
            &request.text,
        );
        self.snapshots.insert(request.buffer_id, snap);
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        self.snapshots
            .get(&buffer_id)
            .filter(|s| s.version == version)
            .cloned()
    }
}

// ── Manager (multiplexer) ─────────────────────────────────────────────────

/// Routes highlight requests to the appropriate backend based on config.
pub struct SyntaxManager {
    config: SyntaxConfig,
    tree_engine: TreeSitterEngine,
    plain_engine: PlainTextEngine,
    buffer_engines: HashMap<usize, EngineKind>,
    statuses: HashMap<usize, SyntaxStatus>,
}

impl SyntaxManager {
    pub fn new(config: SyntaxConfig) -> Self {
        Self {
            config,
            tree_engine: TreeSitterEngine::new(),
            plain_engine: PlainTextEngine::new(),
            buffer_engines: HashMap::new(),
            statuses: HashMap::new(),
        }
    }

    pub fn config(&self) -> &SyntaxConfig {
        &self.config
    }

    pub fn update_config(&mut self, config: SyntaxConfig) {
        self.config = config;
    }

    /// Pre-warm the tree-sitter worker so grammar and parser are ready.
    pub fn prewarm_language(&self, lang: LanguageId) {
        self.tree_engine.prewarm_language(lang);
    }

    /// Return the most recent highlighting status for a buffer.
    pub fn status_for(&self, buffer_id: usize) -> SyntaxStatus {
        if !self.config.enabled {
            return SyntaxStatus::Disabled;
        }
        self.statuses
            .get(&buffer_id)
            .cloned()
            .unwrap_or(SyntaxStatus::Ok(self.config.engine))
    }

    fn set_buffer_engine(&mut self, buffer_id: usize, kind: EngineKind, status: SyntaxStatus) {
        self.statuses.insert(buffer_id, status);
        self.buffer_engines.insert(buffer_id, kind);
    }

    fn engine_for(&self, buffer_id: usize) -> EngineKind {
        self.buffer_engines
            .get(&buffer_id)
            .copied()
            .unwrap_or(EngineKind::Plain)
    }
}

impl SyntaxEngine for SyntaxManager {
    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        if !self.config.enabled {
            self.set_buffer_engine(request.buffer_id, EngineKind::Plain, SyntaxStatus::Disabled);
            return;
        }

        let engine = self.config.engine;
        self.set_buffer_engine(request.buffer_id, engine, SyntaxStatus::Ok(engine));

        match engine {
            EngineKind::Plain => self.plain_engine.schedule_rehighlight(request),
            EngineKind::TreeSitter => self.tree_engine.schedule_rehighlight(request),
        }
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        let snap = match self.engine_for(buffer_id) {
            EngineKind::TreeSitter => self.tree_engine.try_get_highlights(buffer_id, version),
            EngineKind::Plain => self.plain_engine.try_get_highlights(buffer_id, version),
        };
        if let Some(ref s) = snap {
            self.statuses.insert(buffer_id, s.status.clone());
        }
        snap
    }
}
