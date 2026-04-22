//! Multiplexes syntax requests across the plain-text and tree-sitter engines,
//! holding per-buffer metadata so status reporting stays engine-aware.

use std::collections::HashMap;

use crate::highlight::{EngineKind, HighlightSnapshot, SyntaxStatus};
use crate::language::LanguageId;

use super::plain::PlainTextEngine;
use super::tree_sitter::TreeSitterEngine;
use super::{HighlightRequest, SyntaxConfig, SyntaxEngine};

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
            .unwrap_or_else(|| SyntaxStatus::Ok(self.config.engine))
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
        let snapshot = match self.engine_for(buffer_id) {
            EngineKind::TreeSitter => self.tree_engine.try_get_highlights(buffer_id, version),
            EngineKind::Plain => self.plain_engine.try_get_highlights(buffer_id, version),
        };
        // Keep the buffer's public status in sync with whatever the engine just produced.
        if let (Some(s), Some(meta)) = (&snapshot, self.buffers.get_mut(&buffer_id)) {
            meta.status = s.status.clone();
        }
        snapshot
    }
}
