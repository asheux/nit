//! Per-buffer state and the per-job dispatcher: picks between full reparse,
//! viewport-scoped highlight, and incremental update; wraps the actual work
//! in a panic-catching boundary so one bad buffer can't take the worker down.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use nit_core::BufferPoint;
use tracing::{debug, error};
use tree_sitter::{InputEdit, Parser, Point, QueryCursor, Tree};
use tree_sitter_highlight::{HighlightConfiguration, Highlighter};

use crate::captures::QueryConfig;
use crate::engine::HighlightRequest;
use crate::highlight::{EngineKind, HighlightSnapshot, SyntaxStatus};
use crate::language::{LanguageId, LanguageRegistry};

use super::incremental::incremental_highlight;
use super::modes::{full_highlight, viewport_highlight};
use super::progressive::ProgressiveFill;
use super::worker::WorkerState;

pub(super) struct BufferState {
    pub language: LanguageId,
    pub parser: Parser,
    pub tree: Option<Tree>,
    pub snapshot: Option<HighlightSnapshot>,
    pub cursor: QueryCursor,
}

// Single-slot throttle: `f` runs at most once per `interval`. Seeded so the
// first call always fires even before `interval` has elapsed at process start.
// Recovers from lock poison so a panicked logger doesn't silently kill all
// subsequent logging on this slot.
struct RateLimiter {
    state: OnceLock<Mutex<Instant>>,
    interval: Duration,
}

impl RateLimiter {
    const fn new(interval: Duration) -> Self {
        Self {
            state: OnceLock::new(),
            interval,
        }
    }

    fn throttled(&self, f: impl FnOnce()) {
        let now = Instant::now();
        let guard = self.state.get_or_init(|| Mutex::new(now - self.interval));
        let mut last = guard.lock().unwrap_or_else(|p| p.into_inner());
        if now.duration_since(*last) >= self.interval {
            *last = now;
            f();
        }
    }
}

static LOG_COMPLETE: RateLimiter = RateLimiter::new(Duration::from_secs(1));
static LOG_ERROR: RateLimiter = RateLimiter::new(Duration::from_secs(1));

pub(super) fn run_highlight_job(
    job: &HighlightRequest,
    state: &mut WorkerState,
    fills: &mut HashMap<usize, ProgressiveFill>,
) -> HighlightSnapshot {
    let start = Instant::now();
    let WorkerState {
        buffers,
        hl_configs,
        query_configs,
        highlighter,
    } = state;

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        highlight_job(buffers, hl_configs, query_configs, highlighter, job)
    }));

    let mut snapshot = match outcome {
        Ok(Ok(snap)) => {
            log_completion(job.buffer_id, job.version, &snap);
            snap
        }
        Ok(Err(err)) => {
            log_error(job.buffer_id, job.version, &err);
            fallback_snapshot(job, SyntaxStatus::Error(err.to_string()))
        }
        Err(panic_info) => {
            let msg = extract_panic_message(&*panic_info);
            error!(
                buffer_id = job.buffer_id,
                version = job.version,
                "syntax worker panic: {msg}"
            );
            // Drop both the buffer's parser/tree state and any outstanding
            // progressive fill — the fill references the BufferState we just
            // removed, and stepping it would no-op forever otherwise.
            buffers.remove(&job.buffer_id);
            fills.remove(&job.buffer_id);
            fallback_snapshot(job, SyntaxStatus::Error(format!("worker panic: {msg}")))
        }
    };
    snapshot.duration_ms = start.elapsed().as_millis();
    snapshot
}

fn log_completion(buffer_id: usize, version: u64, snapshot: &HighlightSnapshot) {
    LOG_COMPLETE.throttled(|| {
        let span_count: usize = snapshot.per_line.iter().map(|line| line.len()).sum();
        debug!(
            buffer_id,
            version,
            span_count,
            duration_ms = snapshot.duration_ms,
            "syntax highlight complete"
        );
    });
}

fn log_error(buffer_id: usize, version: u64, err: &anyhow::Error) {
    LOG_ERROR.throttled(|| {
        error!(buffer_id, version, error = %err, "syntax highlight error");
    });
}

fn extract_panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic".to_string()
}

fn highlight_job(
    buffers: &mut HashMap<usize, BufferState>,
    hl_configs: &HashMap<LanguageId, HighlightConfiguration>,
    query_configs: &HashMap<LanguageId, QueryConfig>,
    highlighter: &mut Highlighter,
    job: &HighlightRequest,
) -> anyhow::Result<HighlightSnapshot> {
    let lang = job.language;

    let Some(config) = hl_configs.get(&lang) else {
        debug!("no highlight config for {lang:?}");
        return Ok(fallback_snapshot(
            job,
            SyntaxStatus::Error("no highlight config".into()),
        ));
    };

    let state = buffers.entry(job.buffer_id).or_insert_with(|| BufferState {
        language: lang,
        parser: Parser::new(),
        tree: None,
        snapshot: None,
        cursor: QueryCursor::new(),
    });

    if state.language != lang {
        state.language = lang;
        state.tree = None;
        state.snapshot = None;
    }

    if let Some(ts_lang) = LanguageRegistry::tree_sitter_language(lang) {
        state.parser.set_language(ts_lang)?;
    }

    let (tree, edited_old) = parse_job_tree(state, job);
    let Some(tree) = tree else {
        return Ok(fallback_snapshot(
            job,
            SyntaxStatus::Error("parse failed".into()),
        ));
    };

    let snapshot = if should_incremental_update(state, job) {
        if let Some(prev) = state.snapshot.as_ref() {
            incremental_highlight(
                prev,
                edited_old.as_ref(),
                &tree,
                query_configs,
                &mut state.cursor,
                job,
            )
        } else {
            full_highlight(hl_configs, config, highlighter, job)?
        }
    } else if job.viewport.is_some() {
        viewport_highlight(query_configs, &tree, job, &mut state.cursor)?
    } else {
        full_highlight(hl_configs, config, highlighter, job)?
    };

    state.tree = Some(tree);
    state.snapshot = Some(snapshot.clone());
    Ok(snapshot)
}

/// Parses `job.text` using the appropriate strategy:
/// - full reparse (or first parse) => fresh tree
/// - incremental without edits => reuse existing tree
/// - incremental with edits => apply each edit to a clone, reparse against it,
///   and return both the new tree and the pre-parse edited tree for the
///   incremental highlight phase's `changed_ranges` computation.
fn parse_job_tree(state: &mut BufferState, job: &HighlightRequest) -> (Option<Tree>, Option<Tree>) {
    if job.full_reparse || state.tree.is_none() {
        return (state.parser.parse(job.text.as_bytes(), None), None);
    }
    if job.edits.is_empty() {
        return (state.tree.take(), None);
    }

    let mut existing = state.tree.take().unwrap();
    for edit in &job.edits {
        existing.edit(&to_input_edit(edit));
    }
    let edited_old = existing.clone();
    let tree = state
        .parser
        .parse(job.text.as_bytes(), Some(&existing))
        .or(Some(existing));
    (tree, Some(edited_old))
}

pub(super) fn should_incremental_update(state: &BufferState, job: &HighlightRequest) -> bool {
    !job.full_reparse && !job.edits.is_empty() && state.snapshot.is_some() && state.tree.is_some()
}

// Produces a no-span plain snapshot but stamps `EngineKind::TreeSitter` so the
// UI still reflects the engine that *attempted* the job. Used as an error or
// misconfiguration fallback.
pub(super) fn fallback_snapshot(job: &HighlightRequest, status: SyntaxStatus) -> HighlightSnapshot {
    HighlightSnapshot::plain(
        job.buffer_id,
        job.version,
        job.language,
        EngineKind::TreeSitter,
        status,
        &job.text,
    )
}

fn to_input_edit(edit: &nit_core::BufferEdit) -> InputEdit {
    InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: to_point(edit.start_point),
        old_end_position: to_point(edit.old_end_point),
        new_end_position: to_point(edit.new_end_point),
    }
}

fn to_point(point: BufferPoint) -> Point {
    Point::new(point.row, point.column)
}
