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

    /// Transfer a completed snapshot to a renderer that keeps its own copy.
    /// Progressive fills may publish another snapshot for the same version.
    pub fn take_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        let meta = self.buffers.get_mut(&buffer_id)?;
        let cache = match meta.engine {
            EngineKind::TreeSitter => self.tree_engine.snapshots(),
            EngineKind::Plain => &mut self.plain_engine.snapshots,
        };
        let std::collections::hash_map::Entry::Occupied(entry) = cache.entry(buffer_id) else {
            return None;
        };
        if entry.get().version != version {
            return None;
        }
        let snapshot = entry.remove();
        meta.status = snapshot.status.clone();
        Some(snapshot)
    }
}

impl SyntaxEngine for SyntaxManager {
    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        if !self.config.enabled {
            self.buffers.insert(
                request.buffer_id,
                BufferMeta {
                    engine: EngineKind::Plain,
                    status: SyntaxStatus::Disabled,
                },
            );
            return;
        }

        let engine = self.config.engine;
        self.buffers.insert(
            request.buffer_id,
            BufferMeta {
                engine,
                status: SyntaxStatus::Ok(engine),
            },
        );

        match engine {
            EngineKind::Plain => self.plain_engine.schedule_rehighlight(request),
            EngineKind::TreeSitter => self.tree_engine.schedule_rehighlight(request),
        }
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        let engine = self
            .buffers
            .get(&buffer_id)
            .map_or(EngineKind::Plain, |m| m.engine);
        let snapshot = match engine {
            EngineKind::TreeSitter => self.tree_engine.try_get_highlights(buffer_id, version),
            EngineKind::Plain => self.plain_engine.try_get_highlights(buffer_id, version),
        };
        if let (Some(s), Some(meta)) = (&snapshot, self.buffers.get_mut(&buffer_id)) {
            meta.status = s.status.clone();
        }
        snapshot
    }
}
