//! Plain-text fallback engine: produces a no-span snapshot by tracking
//! line offsets and hashes, used when tree-sitter is disabled or unavailable.

use std::collections::HashMap;

use crate::highlight::{EngineKind, HighlightSnapshot, SyntaxStatus};

use super::{HighlightRequest, SyntaxEngine};

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
