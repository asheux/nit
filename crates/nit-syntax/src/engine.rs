use std::collections::HashMap;
use std::path::Path;

use nit_core::BufferEdit;

use crate::highlight::{EngineKind, HighlightSnapshot, SyntaxStatus};
use crate::registry::{LanguageId, LanguageRegistry};
use crate::tree_sitter_engine::TreeSitterEngine;

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

#[derive(Clone, Debug)]
pub struct HighlightRequest {
    pub buffer_id: usize,
    pub version: u64,
    pub language: LanguageId,
    pub text: String,
    pub edits: Vec<BufferEdit>,
    pub full_reparse: bool,
    pub max_spans_per_line: usize,
}

pub trait SyntaxEngine {
    fn detect_language(
        &self,
        path: Option<&Path>,
        first_line: Option<&str>,
        override_lang: Option<LanguageId>,
    ) -> LanguageId;
    fn schedule_rehighlight(&mut self, request: HighlightRequest);
    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot>;
}

pub struct PlainTextEngine {
    snapshots: HashMap<usize, HighlightSnapshot>,
}

impl PlainTextEngine {
    pub fn new() -> Self {
        Self {
            snapshots: HashMap::new(),
        }
    }
}

impl Default for PlainTextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxEngine for PlainTextEngine {
    fn detect_language(
        &self,
        path: Option<&Path>,
        first_line: Option<&str>,
        override_lang: Option<LanguageId>,
    ) -> LanguageId {
        LanguageRegistry::detect(path, first_line, override_lang)
    }

    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        let snapshot = HighlightSnapshot::plain(
            request.buffer_id,
            request.version,
            request.language,
            EngineKind::Plain,
            SyntaxStatus::Ok(EngineKind::Plain),
            &request.text,
        );
        self.snapshots.insert(request.buffer_id, snapshot);
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        self.snapshots
            .get(&buffer_id)
            .filter(|s| s.version == version)
            .cloned()
    }
}

pub struct SyntaxManager {
    config: SyntaxConfig,
    tree: TreeSitterEngine,
    plain: PlainTextEngine,
    engine_for_buffer: HashMap<usize, EngineKind>,
    status: HashMap<usize, SyntaxStatus>,
}

impl SyntaxManager {
    pub fn new(config: SyntaxConfig) -> Self {
        Self {
            config,
            tree: TreeSitterEngine::new(),
            plain: PlainTextEngine::new(),
            engine_for_buffer: HashMap::new(),
            status: HashMap::new(),
        }
    }

    pub fn config(&self) -> &SyntaxConfig {
        &self.config
    }

    pub fn update_config(&mut self, config: SyntaxConfig) {
        self.config = config;
    }

    pub fn status_for(&self, buffer_id: usize) -> SyntaxStatus {
        if !self.config.enabled {
            return SyntaxStatus::Disabled;
        }
        if self.config.engine == EngineKind::Plain {
            return self
                .status
                .get(&buffer_id)
                .cloned()
                .unwrap_or(SyntaxStatus::Ok(EngineKind::Plain));
        }
        self.status
            .get(&buffer_id)
            .cloned()
            .unwrap_or(SyntaxStatus::Ok(EngineKind::TreeSitter))
    }
}

impl SyntaxEngine for SyntaxManager {
    fn detect_language(
        &self,
        path: Option<&Path>,
        first_line: Option<&str>,
        override_lang: Option<LanguageId>,
    ) -> LanguageId {
        LanguageRegistry::detect(path, first_line, override_lang)
    }

    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        if !self.config.enabled {
            self.status
                .insert(request.buffer_id, SyntaxStatus::Disabled);
            self.engine_for_buffer
                .insert(request.buffer_id, EngineKind::Plain);
            return;
        }

        if self.config.engine == EngineKind::Plain {
            self.status
                .insert(request.buffer_id, SyntaxStatus::Ok(EngineKind::Plain));
            self.engine_for_buffer
                .insert(request.buffer_id, EngineKind::Plain);
            self.plain.schedule_rehighlight(request);
            return;
        }

        if request.text.len() > self.config.max_file_bytes {
            self.status
                .insert(request.buffer_id, SyntaxStatus::LargeFile);
            self.engine_for_buffer
                .insert(request.buffer_id, EngineKind::Plain);
            self.plain.schedule_rehighlight(request);
            return;
        }

        self.status
            .insert(request.buffer_id, SyntaxStatus::Ok(EngineKind::TreeSitter));
        self.engine_for_buffer
            .insert(request.buffer_id, EngineKind::TreeSitter);
        self.tree.schedule_rehighlight(request);
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        let engine = self
            .engine_for_buffer
            .get(&buffer_id)
            .copied()
            .unwrap_or(EngineKind::Plain);
        let snapshot = match engine {
            EngineKind::TreeSitter => self.tree.try_get_highlights(buffer_id, version),
            EngineKind::Plain => self.plain.try_get_highlights(buffer_id, version),
        };
        if let Some(ref snap) = snapshot {
            let keep_large = matches!(self.status.get(&buffer_id), Some(SyntaxStatus::LargeFile));
            if !keep_large {
                self.status.insert(buffer_id, snap.status.clone());
            }
        }
        snapshot
    }
}
