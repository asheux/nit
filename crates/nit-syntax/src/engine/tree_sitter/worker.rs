//! Worker-thread lifecycle: pulls [`HighlightRequest`]s, deduplicates
//! them per-buffer, runs the job pipeline, and drives any outstanding
//! progressive-fill work.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};

use tree_sitter_highlight::{HighlightConfiguration, Highlighter};

use crate::captures::{build_highlight_configs, build_query_configs, QueryConfig};
use crate::engine::HighlightRequest;
use crate::highlight::HighlightSnapshot;
use crate::language::LanguageId;

use super::job::{run_highlight_job, BufferState};
use super::progressive::{
    make_progressive_fill, step_progressive_fills, ProgressiveFill, FILL_IDLE_TIMEOUT,
};

pub(super) struct HighlightResult {
    pub buffer_id: usize,
    pub snapshot: HighlightSnapshot,
}

pub(super) struct WorkerState {
    pub buffers: HashMap<usize, BufferState>,
    pub hl_configs: HashMap<LanguageId, HighlightConfiguration>,
    pub query_configs: HashMap<LanguageId, QueryConfig>,
    pub highlighter: Highlighter,
}

pub(super) fn worker_loop(rx: Receiver<HighlightRequest>, res_tx: Sender<HighlightResult>) {
    let mut state = WorkerState {
        buffers: HashMap::new(),
        hl_configs: build_highlight_configs(),
        query_configs: build_query_configs(),
        highlighter: Highlighter::new(),
    };
    let mut fills: HashMap<usize, ProgressiveFill> = HashMap::new();

    loop {
        let initial = match next_request(&rx, !fills.is_empty()) {
            RequestPoll::Got(req) => Some(req),
            RequestPoll::Idle => None,
            RequestPoll::Disconnected => break,
        };

        if let Some(first) = initial {
            let jobs = drain_pending(first, &rx);

            for job in &jobs {
                fills.remove(&job.buffer_id);
                let snapshot = run_highlight_job(job, &mut state, &mut fills);

                if let Some(fill) = make_progressive_fill(job, &snapshot) {
                    fills.insert(job.buffer_id, fill);
                }

                let _ = res_tx.send(HighlightResult {
                    buffer_id: job.buffer_id,
                    snapshot,
                });
            }
        }

        step_progressive_fills(&mut fills, &mut state, &res_tx);
    }
}

enum RequestPoll {
    Got(HighlightRequest),
    Idle,
    Disconnected,
}

fn next_request(rx: &Receiver<HighlightRequest>, has_pending_fills: bool) -> RequestPoll {
    if has_pending_fills {
        match rx.recv_timeout(FILL_IDLE_TIMEOUT) {
            Ok(req) => RequestPoll::Got(req),
            Err(mpsc::RecvTimeoutError::Timeout) => RequestPoll::Idle,
            Err(mpsc::RecvTimeoutError::Disconnected) => RequestPoll::Disconnected,
        }
    } else {
        match rx.recv() {
            Ok(req) => RequestPoll::Got(req),
            Err(_) => RequestPoll::Disconnected,
        }
    }
}

/// Batch-drain all queued requests, keeping only the latest per buffer, and
/// return them sorted so `full_reparse` jobs run before incremental ones.
fn drain_pending(
    first: HighlightRequest,
    rx: &Receiver<HighlightRequest>,
) -> Vec<HighlightRequest> {
    let mut pending: HashMap<usize, HighlightRequest> = HashMap::new();
    pending.insert(first.buffer_id, first);
    while let Ok(job) = rx.try_recv() {
        pending.insert(job.buffer_id, job);
    }
    let mut jobs: Vec<HighlightRequest> = pending.into_values().collect();
    jobs.sort_by_key(|j| if j.full_reparse { 0u8 } else { 1 });
    jobs
}
