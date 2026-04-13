//! Syntax engine trait, configuration, and manager.

use std::collections::HashMap;
use std::path::Path;

use nit_core::BufferEdit;

use crate::highlight::{EngineKind, HighlightSnapshot, SyntaxStatus};
use crate::registry::{LanguageId, LanguageRegistry};
use crate::tree_sitter_engine::TreeSitterEngine;

// ── Configuration ─────────────────────────────────────────────────────────

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

// ── Viewport ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViewportRange {
    pub first_line: usize,
    pub last_line: usize,
    pub total_lines: usize,
}

// ── Highlight request ─────────────────────────────────────────────────────

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

// ── Plain-text fallback engine ────────────────────────────────────────────

#[derive(Default)]
pub struct PlainTextEngine {
    snapshots: HashMap<usize, HighlightSnapshot>,
}

impl PlainTextEngine {
    #[must_use]
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

struct BufferMeta {
    engine: EngineKind,
    status: SyntaxStatus,
}

pub struct SyntaxManager {
    config: SyntaxConfig,
    tree_engine: TreeSitterEngine,
    plain_engine: PlainTextEngine,
    buffers: HashMap<usize, BufferMeta>,
}

impl SyntaxManager {
    #[must_use]
    pub fn new(config: SyntaxConfig) -> Self {
        Self {
            config,
            tree_engine: TreeSitterEngine::new(),
            plain_engine: PlainTextEngine::default(),
            buffers: HashMap::new(),
        }
    }

    #[must_use]
    pub fn config(&self) -> &SyntaxConfig {
        &self.config
    }

    pub fn update_config(&mut self, config: SyntaxConfig) {
        self.config = config;
    }

    pub fn prewarm_language(&self, lang: LanguageId) {
        self.tree_engine.prewarm_language(lang);
    }

    #[must_use]
    pub fn status_for(&self, buffer_id: usize) -> SyntaxStatus {
        if !self.config.enabled {
            return SyntaxStatus::Disabled;
        }
        self.buffers
            .get(&buffer_id)
            .map(|m| m.status.clone())
            .unwrap_or(SyntaxStatus::Ok(self.config.engine))
    }

    fn set_buffer_meta(&mut self, buffer_id: usize, engine: EngineKind, status: SyntaxStatus) {
        self.buffers
            .insert(buffer_id, BufferMeta { engine, status });
    }

    fn engine_for(&self, buffer_id: usize) -> EngineKind {
        self.buffers
            .get(&buffer_id)
            .map_or(EngineKind::Plain, |m| m.engine)
    }
}

impl SyntaxEngine for SyntaxManager {
    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        if !self.config.enabled {
            self.set_buffer_meta(request.buffer_id, EngineKind::Plain, SyntaxStatus::Disabled);
            return;
        }

        let engine = self.config.engine;
        self.set_buffer_meta(request.buffer_id, engine, SyntaxStatus::Ok(engine));

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
        if let (Some(s), Some(meta)) = (&snap, self.buffers.get_mut(&buffer_id)) {
            meta.status = s.status.clone();
        }
        snap
    }
}
