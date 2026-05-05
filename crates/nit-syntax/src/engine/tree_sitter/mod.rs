//! Background-threaded tree-sitter worker: accepts [`HighlightRequest`]s,
//! produces [`HighlightSnapshot`]s, and progressively fills viewport-scoped
//! jobs once the visible region is ready.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::highlight::HighlightSnapshot;
use crate::language::LanguageId;

use super::{HighlightRequest, SyntaxEngine};

mod incremental;
mod job;
mod modes;
mod progressive;
mod worker;

use worker::{worker_loop, HighlightResult};

pub struct TreeSitterEngine {
    req_tx: Sender<HighlightRequest>,
    res_rx: Receiver<HighlightResult>,
    cache: HashMap<usize, HighlightSnapshot>,
}

impl TreeSitterEngine {
    pub fn new() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<HighlightRequest>();
        let (res_tx, res_rx) = mpsc::channel::<HighlightResult>();
        thread::spawn(move || worker_loop(req_rx, res_tx));
        Self {
            req_tx,
            res_rx,
            cache: HashMap::new(),
        }
    }

    pub fn prewarm_language(&self, lang: LanguageId) {
        let _ = self.req_tx.send(HighlightRequest::prewarm(lang));
    }
}

impl SyntaxEngine for TreeSitterEngine {
    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        let _ = self.req_tx.send(request);
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        while let Ok(r) = self.res_rx.try_recv() {
            self.cache.insert(r.buffer_id, r.snapshot);
        }
        self.cache
            .get(&buffer_id)
            .filter(|s| s.version == version)
            .cloned()
    }
}
