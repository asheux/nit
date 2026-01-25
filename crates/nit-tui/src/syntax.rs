use std::collections::HashMap;

use nit_core::{Buffer, BufferEdit, HighlightConfig, HighlightEngine};
use nit_syntax::{Debouncer, HighlightRequest, HighlightSnapshot, LanguageId, SyntaxEngine, SyntaxManager};

#[derive(Debug, Clone)]
struct PendingSyntax {
    version: u64,
    language: LanguageId,
    edits: Vec<BufferEdit>,
    full_reparse: bool,
}

pub struct SyntaxRuntime {
    manager: SyntaxManager,
    debouncers: HashMap<usize, Debouncer>,
    pending: HashMap<usize, PendingSyntax>,
    snapshots: HashMap<usize, HighlightSnapshot>,
}

impl SyntaxRuntime {
    pub fn new(config: HighlightConfig) -> Self {
        let manager = SyntaxManager::new(config_to_syntax(config));
        Self {
            manager,
            debouncers: HashMap::new(),
            pending: HashMap::new(),
            snapshots: HashMap::new(),
        }
    }

    pub fn update_config(&mut self, config: HighlightConfig) {
        let debounce = config.debounce_ms;
        self.manager.update_config(config_to_syntax(config));
        for debouncer in self.debouncers.values_mut() {
            *debouncer = Debouncer::new(debounce);
        }
        if !self.manager.config().enabled {
            self.pending.clear();
            self.snapshots.clear();
        }
    }

    pub fn prime_buffer(&mut self, buffer_id: usize, buffer: &Buffer) {
        let first_line = buffer.first_line();
        let language = self.manager.detect_language(
            buffer.path().map(|p| p.as_path()),
            first_line.as_deref(),
            None,
        );
        let pending = PendingSyntax {
            version: buffer.version(),
            language,
            edits: Vec::new(),
            full_reparse: true,
        };
        self.pending.insert(buffer_id, pending);
        let debouncer = self
            .debouncers
            .entry(buffer_id)
            .or_insert_with(|| Debouncer::new(self.manager.config().debounce_ms));
        debouncer.mark();
    }

    pub fn note_buffer_change(&mut self, buffer_id: usize, buffer: &mut Buffer) {
        let edits = buffer.take_pending_edits();
        let full_reparse = buffer.take_full_reparse();
        if edits.is_empty() && !full_reparse {
            return;
        }
        let first_line = buffer.first_line();
        let language = self.manager.detect_language(
            buffer.path().map(|p| p.as_path()),
            first_line.as_deref(),
            None,
        );
        let entry = self.pending.entry(buffer_id).or_insert(PendingSyntax {
            version: buffer.version(),
            language,
            edits: Vec::new(),
            full_reparse,
        });
        entry.version = buffer.version();
        entry.language = language;
        if entry.full_reparse || full_reparse {
            entry.full_reparse = true;
            entry.edits.clear();
        } else {
            entry.edits.extend(edits);
        }
        let debouncer = self
            .debouncers
            .entry(buffer_id)
            .or_insert_with(|| Debouncer::new(self.manager.config().debounce_ms));
        debouncer.mark();
    }

    pub fn tick(&mut self, buffer_id: usize, buffer: &Buffer) {
        if !self.manager.config().enabled {
            self.pending.remove(&buffer_id);
            if let Some(debouncer) = self.debouncers.get_mut(&buffer_id) {
                debouncer.clear();
            }
            return;
        }
        let Some(debouncer) = self.debouncers.get_mut(&buffer_id) else {
            return;
        };
        if !debouncer.ready() {
            return;
        }
        let Some(pending) = self.pending.remove(&buffer_id) else {
            debouncer.clear();
            return;
        };
        let text = buffer.content_as_string();
        let request = HighlightRequest {
            buffer_id,
            version: pending.version,
            language: pending.language,
            text,
            edits: pending.edits,
            full_reparse: pending.full_reparse,
            max_spans_per_line: self.manager.config().max_spans_per_line,
        };
        self.manager.schedule_rehighlight(request);
        debouncer.clear();
    }

    pub fn poll_results(&mut self, buffer_id: usize, version: u64) {
        if let Some(snapshot) = self.manager.try_get_highlights(buffer_id, version) {
            self.snapshots.insert(buffer_id, snapshot);
        }
    }

    pub fn snapshot_for(&self, buffer_id: usize, version: u64) -> Option<&HighlightSnapshot> {
        self.snapshots
            .get(&buffer_id)
            .filter(|snap| snap.version == version)
    }

    pub fn status_label(&self, buffer_id: usize) -> String {
        self.manager.status_for(buffer_id).label()
    }
}

fn config_to_syntax(config: HighlightConfig) -> nit_syntax::SyntaxConfig {
    let engine = match config.engine {
        HighlightEngine::TreeSitter => nit_syntax::EngineKind::TreeSitter,
        HighlightEngine::Plain => nit_syntax::EngineKind::Plain,
    };
    nit_syntax::SyntaxConfig {
        enabled: config.enabled,
        engine,
        debounce_ms: config.debounce_ms,
        max_file_bytes: config.max_file_bytes,
        max_spans_per_line: config.max_spans_per_line,
    }
}
