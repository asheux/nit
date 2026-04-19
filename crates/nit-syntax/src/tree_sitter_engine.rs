//! Background-threaded tree-sitter worker: accepts [`HighlightRequest`]s,
//! produces [`HighlightSnapshot`]s, and progressively fills viewport-scoped
//! jobs once the visible region is ready.

use std::cmp;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use nit_core::{BufferEdit, BufferPoint};
use tracing::{debug, error};
use tree_sitter::{InputEdit, Parser, Point, QueryCursor, Tree};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::captures::{build_highlight_configs, build_query_configs, capture_group, QueryConfig};
use crate::engine::{HighlightRequest, SyntaxEngine};
use crate::highlight::{
    compute_line_starts, distribute_spans_to_lines, find_line, recompute_line_hashes, sort_spans,
    EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan, SyntaxStatus,
};
use crate::registry::{LanguageId, LanguageRegistry};

// Amount by which viewport-scoped highlights extend beyond the visible range,
// so small scroll jitter doesn't immediately require a re-request.
const VIEWPORT_MARGIN_LINES: usize = 100;
// Chunk size for progressive fill beyond the initial viewport window.
const FILL_CHUNK_LINES: usize = 500;
// Worker wakes up to drive progressive fill even when no request arrives.
const FILL_IDLE_TIMEOUT: Duration = Duration::from_millis(10);

pub struct TreeSitterEngine {
    req_tx: Sender<HighlightRequest>,
    res_rx: Receiver<HighlightResult>,
    cache: HashMap<usize, HighlightSnapshot>,
}

struct HighlightResult {
    buffer_id: usize,
    snapshot: HighlightSnapshot,
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
        let _ = self.req_tx.send(HighlightRequest {
            buffer_id: usize::MAX,
            version: 0,
            language: lang,
            text: String::new(),
            edits: Vec::new(),
            full_reparse: true,
            max_spans_per_line: 0,
            viewport: None,
        });
    }

    fn drain_results(&mut self) {
        while let Ok(r) = self.res_rx.try_recv() {
            self.cache.insert(r.buffer_id, r.snapshot);
        }
    }
}

impl SyntaxEngine for TreeSitterEngine {
    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        let _ = self.req_tx.send(request);
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        self.drain_results();
        self.cache
            .get(&buffer_id)
            .filter(|s| s.version == version)
            .cloned()
    }
}

struct BufferState {
    language: LanguageId,
    parser: Parser,
    tree: Option<Tree>,
    snapshot: Option<HighlightSnapshot>,
    cursor: QueryCursor,
}

/// Outstanding progressive-fill work for a viewport-scoped job: expands
/// below the initial range first, then above, chunk by chunk.
struct ProgressiveFill {
    buffer_id: usize,
    version: u64,
    language: LanguageId,
    text: String,
    line_start_bytes: Vec<usize>,
    next_below: Option<usize>,
    next_above: Option<usize>,
    chunk_size: usize,
    total_lines: usize,
    max_spans_per_line: usize,
}

struct WorkerState {
    buffers: HashMap<usize, BufferState>,
    hl_configs: HashMap<LanguageId, HighlightConfiguration>,
    query_configs: HashMap<LanguageId, QueryConfig>,
    highlighter: Highlighter,
}

fn worker_loop(rx: Receiver<HighlightRequest>, res_tx: Sender<HighlightResult>) {
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
                let snapshot = run_highlight_job(job, &mut state);

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

fn step_progressive_fills(
    fills: &mut HashMap<usize, ProgressiveFill>,
    state: &mut WorkerState,
    res_tx: &Sender<HighlightResult>,
) {
    let ids: Vec<usize> = fills.keys().copied().collect();
    for id in ids {
        let Some(fill) = fills.get_mut(&id) else {
            continue;
        };
        if process_fill_chunk(fill, &mut state.buffers, &state.query_configs, res_tx) {
            fills.remove(&id);
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

fn run_highlight_job(job: &HighlightRequest, state: &mut WorkerState) -> HighlightSnapshot {
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
            let msg = extract_panic_message(&panic_info);
            error!(
                buffer_id = job.buffer_id,
                version = job.version,
                "syntax worker panic: {msg}"
            );
            buffers.remove(&job.buffer_id);
            fallback_snapshot(job, SyntaxStatus::Error(format!("worker panic: {msg}")))
        }
    };
    snapshot.duration_ms = start.elapsed().as_millis();
    snapshot
}

fn make_progressive_fill(
    job: &HighlightRequest,
    snapshot: &HighlightSnapshot,
) -> Option<ProgressiveFill> {
    let (hl_start, hl_end) = snapshot.highlighted_range?;
    let total = snapshot.per_line.len();

    if hl_end + 1 >= total && hl_start == 0 {
        return None;
    }

    Some(ProgressiveFill {
        buffer_id: job.buffer_id,
        version: job.version,
        language: job.language,
        text: job.text.clone(),
        line_start_bytes: snapshot.line_start_bytes.clone(),
        next_below: (hl_end + 1 < total).then_some(hl_end + 1),
        next_above: (hl_start > 0).then_some(hl_start),
        chunk_size: FILL_CHUNK_LINES,
        total_lines: total,
        max_spans_per_line: job.max_spans_per_line,
    })
}

// Produces a no-span plain snapshot but stamps `EngineKind::TreeSitter` so the
// UI still reflects the engine that *attempted* the job. Used as an error or
// misconfiguration fallback.
fn fallback_snapshot(job: &HighlightRequest, status: SyntaxStatus) -> HighlightSnapshot {
    HighlightSnapshot::plain(
        job.buffer_id,
        job.version,
        job.language,
        EngineKind::TreeSitter,
        status,
        &job.text,
    )
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

fn viewport_highlight(
    query_configs: &HashMap<LanguageId, QueryConfig>,
    tree: &Tree,
    job: &HighlightRequest,
    cursor: &mut QueryCursor,
) -> anyhow::Result<HighlightSnapshot> {
    let viewport = job.viewport.as_ref().unwrap();
    let offsets = compute_line_starts(&job.text);
    let total = offsets.len().saturating_sub(1);

    let start_line = viewport.first_line.saturating_sub(VIEWPORT_MARGIN_LINES);
    let end_line = (viewport.last_line + VIEWPORT_MARGIN_LINES).min(total.saturating_sub(1));

    let start_byte = offsets[start_line];
    let end_byte = offsets.get(end_line + 1).copied().unwrap_or(job.text.len());

    let Some(cfg) = query_configs.get(&job.language) else {
        return Ok(fallback_snapshot(
            job,
            SyntaxStatus::Error("no query config".into()),
        ));
    };

    let mut spans = Vec::new();
    collect_spans(
        cfg,
        tree,
        job.text.as_bytes(),
        &[(start_byte, end_byte)],
        &mut spans,
        cursor,
    );
    sort_spans(&mut spans);

    let mut per_line = vec![Vec::new(); total];
    distribute_spans_to_lines(
        &spans,
        &offsets,
        &mut per_line,
        job.max_spans_per_line,
        |_| true,
    );

    let mut line_hashes = vec![0u64; total];
    recompute_line_hashes(
        job.text.as_bytes(),
        &offsets,
        &mut line_hashes,
        start_line..=end_line.min(total.saturating_sub(1)),
    );

    Ok(HighlightSnapshot {
        buffer_id: job.buffer_id,
        version: job.version,
        language: job.language,
        engine: EngineKind::TreeSitter,
        status: SyntaxStatus::Ok(EngineKind::TreeSitter),
        duration_ms: 0,
        line_start_bytes: offsets,
        line_hashes,
        per_line,
        highlighted_range: Some((start_line, end_line)),
    })
}

fn full_highlight(
    configs: &HashMap<LanguageId, HighlightConfiguration>,
    config: &HighlightConfiguration,
    highlighter: &mut Highlighter,
    job: &HighlightRequest,
) -> anyhow::Result<HighlightSnapshot> {
    let mut spans = Vec::new();
    let mut stack: Vec<HighlightGroup> = Vec::new();

    let iter = highlighter.highlight(config, job.text.as_bytes(), None, |name| {
        LanguageRegistry::from_injection_name(name).and_then(|id| configs.get(&id))
    })?;

    for event in iter {
        match event? {
            HighlightEvent::HighlightStart(s) => stack.push(capture_group(s.0)),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let group = stack.last().copied().unwrap_or(HighlightGroup::Normal);
                if group != HighlightGroup::Normal && end > start {
                    spans.push(HighlightSpan {
                        start_byte: start,
                        end_byte: end,
                        group,
                        priority: 0,
                        modifiers: 0,
                    });
                }
            }
        }
    }

    Ok(HighlightSnapshot::from_spans(
        job.buffer_id,
        job.version,
        job.language,
        EngineKind::TreeSitter,
        SyntaxStatus::Ok(EngineKind::TreeSitter),
        &job.text,
        spans,
        job.max_spans_per_line,
    ))
}

fn should_incremental_update(state: &BufferState, job: &HighlightRequest) -> bool {
    !job.full_reparse && !job.edits.is_empty() && state.snapshot.is_some() && state.tree.is_some()
}

/// Re-highlight only the lines affected by edits, reusing the rest of the
/// previous snapshot. Flow: carry forward unchanged lines from `prev`, mark
/// lines touched by `changed_ranges` plus any that couldn't be mapped as
/// dirty, then re-collect spans and hashes for just those dirty lines.
fn incremental_highlight(
    prev: &HighlightSnapshot,
    edited_old: Option<&Tree>,
    tree: &Tree,
    query_configs: &HashMap<LanguageId, QueryConfig>,
    cursor: &mut QueryCursor,
    job: &HighlightRequest,
) -> HighlightSnapshot {
    let offsets = compute_line_starts(&job.text);
    let line_count = offsets.len().saturating_sub(1);

    let (mut per_line, mut line_hashes, copied) = carry_forward_lines(prev, &job.edits, line_count);

    let dirty = compute_dirty_lines(&copied, edited_old, tree, &offsets, line_count);

    if dirty.contains(&true) {
        rehighlight_dirty(
            &dirty,
            &offsets,
            &mut per_line,
            &mut line_hashes,
            query_configs,
            tree,
            cursor,
            job,
        );
    }

    HighlightSnapshot {
        buffer_id: job.buffer_id,
        version: job.version,
        language: job.language,
        engine: EngineKind::TreeSitter,
        status: SyntaxStatus::Ok(EngineKind::TreeSitter),
        duration_ms: 0,
        line_start_bytes: offsets,
        line_hashes,
        per_line,
        highlighted_range: None,
    }
}

/// Clone each old line's segments/hash into the new layout via the edit-shifted
/// line map. Returns `(per_line, line_hashes, copied)`, where `copied[i]` is
/// true iff line `i` received content from the previous snapshot.
fn carry_forward_lines(
    prev: &HighlightSnapshot,
    edits: &[BufferEdit],
    line_count: usize,
) -> (Vec<Vec<crate::highlight::LineSegment>>, Vec<u64>, Vec<bool>) {
    let mut per_line = vec![Vec::new(); line_count];
    let mut line_hashes = vec![0u64; line_count];
    let mut copied = vec![false; line_count];

    let line_map = build_line_map(prev.per_line.len(), edits);
    for (old_i, new_i) in line_map.into_iter().enumerate() {
        let Some(new_i) = new_i else { continue };
        if new_i >= line_count {
            continue;
        }
        per_line[new_i] = prev.per_line[old_i].clone();
        if let Some(&hash) = prev.line_hashes.get(old_i) {
            line_hashes[new_i] = hash;
        }
        copied[new_i] = true;
    }

    (per_line, line_hashes, copied)
}

/// Mark a line as dirty when either: tree-sitter's `changed_ranges` report
/// touches it (with a one-line bleed on each side to cover multi-line tokens),
/// or when no previous content could be carried forward to it.
fn compute_dirty_lines(
    copied: &[bool],
    edited_old: Option<&Tree>,
    tree: &Tree,
    offsets: &[usize],
    line_count: usize,
) -> Vec<bool> {
    let mut dirty = vec![false; line_count];

    if let Some(old_tree) = edited_old {
        for range in old_tree.changed_ranges(tree) {
            if range.end_byte == 0 || line_count == 0 {
                continue;
            }
            let start = find_line(offsets, range.start_byte).saturating_sub(1);
            let end = cmp::min(
                find_line(offsets, range.end_byte.saturating_sub(1)).saturating_add(1),
                line_count.saturating_sub(1),
            );
            let bound = end.saturating_add(1).min(dirty.len());
            for slot in dirty.iter_mut().take(bound).skip(start) {
                *slot = true;
            }
        }
    }

    for (i, &was_copied) in copied.iter().enumerate() {
        if !was_copied {
            dirty[i] = true;
        }
    }

    dirty
}

#[allow(clippy::too_many_arguments)]
fn rehighlight_dirty(
    dirty: &[bool],
    offsets: &[usize],
    per_line: &mut [Vec<crate::highlight::LineSegment>],
    line_hashes: &mut [u64],
    query_configs: &HashMap<LanguageId, QueryConfig>,
    tree: &Tree,
    cursor: &mut QueryCursor,
    job: &HighlightRequest,
) {
    for (i, &is_dirty) in dirty.iter().enumerate() {
        if is_dirty {
            per_line[i].clear();
        }
    }

    if let Some(cfg) = query_configs.get(&job.language) {
        let mut spans = Vec::new();
        let ranges = dirty_byte_ranges(dirty, offsets);
        collect_spans(cfg, tree, job.text.as_bytes(), &ranges, &mut spans, cursor);
        sort_spans(&mut spans);
        distribute_spans_to_lines(&spans, offsets, per_line, job.max_spans_per_line, |line| {
            dirty[line]
        });
    }

    recompute_line_hashes(
        job.text.as_bytes(),
        offsets,
        line_hashes,
        dirty.iter().enumerate().filter(|(_, &d)| d).map(|(i, _)| i),
    );
}

fn process_fill_chunk(
    fill: &mut ProgressiveFill,
    buffers: &mut HashMap<usize, BufferState>,
    query_configs: &HashMap<LanguageId, QueryConfig>,
    res_tx: &Sender<HighlightResult>,
) -> bool {
    let (start_line, end_line) = match next_fill_chunk(fill) {
        Some(range) => range,
        None => return true,
    };

    let Some(cfg) = query_configs.get(&fill.language) else {
        return true;
    };
    let Some(state) = buffers.get_mut(&fill.buffer_id) else {
        return true;
    };
    let Some(tree) = state.tree.as_ref() else {
        return true;
    };
    let Some(snapshot) = state.snapshot.as_mut() else {
        return true;
    };
    if snapshot.version != fill.version {
        return true;
    }

    let start_byte = fill.line_start_bytes[start_line];
    let end_byte = fill
        .line_start_bytes
        .get(end_line + 1)
        .copied()
        .unwrap_or(fill.text.len());

    let mut spans = Vec::new();
    collect_spans(
        cfg,
        tree,
        fill.text.as_bytes(),
        &[(start_byte, end_byte)],
        &mut spans,
        &mut state.cursor,
    );

    sort_spans(&mut spans);
    distribute_spans_to_lines(
        &spans,
        &fill.line_start_bytes,
        &mut snapshot.per_line,
        fill.max_spans_per_line,
        |line| line >= start_line && line <= end_line,
    );

    recompute_line_hashes(
        fill.text.as_bytes(),
        &fill.line_start_bytes,
        &mut snapshot.line_hashes,
        start_line..=end_line,
    );

    expand_highlighted_range(snapshot, fill, start_line, end_line);

    let _ = res_tx.send(HighlightResult {
        buffer_id: fill.buffer_id,
        snapshot: snapshot.clone(),
    });

    fill.next_below.is_none() && fill.next_above.is_none()
}

// Advance the fill cursor, filling downward first and then upward. The
// `next_below`/`next_above` options are updated so that subsequent calls
// pick up where this one left off.
fn next_fill_chunk(fill: &mut ProgressiveFill) -> Option<(usize, usize)> {
    if let Some(next) = fill.next_below {
        let end = (next + fill.chunk_size).min(fill.total_lines);
        fill.next_below = (end < fill.total_lines).then_some(end);
        return Some((next, end.saturating_sub(1)));
    }
    if let Some(above_end) = fill.next_above {
        let start = above_end.saturating_sub(fill.chunk_size);
        fill.next_above = (start > 0).then_some(start);
        return Some((start, above_end.saturating_sub(1)));
    }
    None
}

fn expand_highlighted_range(
    snapshot: &mut HighlightSnapshot,
    fill: &ProgressiveFill,
    start_line: usize,
    end_line: usize,
) {
    let (prev_start, prev_end) = snapshot
        .highlighted_range
        .unwrap_or((0, fill.total_lines.saturating_sub(1)));
    let new_start = prev_start.min(start_line);
    let new_end = prev_end.max(end_line);

    let fully_covered = new_start == 0 && new_end >= fill.total_lines.saturating_sub(1);
    snapshot.highlighted_range = if fully_covered {
        None
    } else {
        Some((new_start, new_end))
    };
}

fn collect_spans(
    cfg: &QueryConfig,
    tree: &Tree,
    source: &[u8],
    ranges: &[(usize, usize)],
    spans: &mut Vec<HighlightSpan>,
    cursor: &mut QueryCursor,
) {
    let root = tree.root_node();
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        cursor.set_byte_range(start..end);
        for m in cursor.matches(&cfg.query, root, source) {
            let priority = cmp::min(m.pattern_index, u8::MAX as usize) as u8;
            for cap in m.captures {
                let group = cfg.highlight_for_index(cap.index as usize);
                if group == HighlightGroup::Normal {
                    continue;
                }
                let node = cap.node;
                let span_start = node.start_byte().max(start);
                let span_end = node.end_byte().min(end);
                if span_start < span_end {
                    spans.push(HighlightSpan {
                        start_byte: span_start,
                        end_byte: span_end,
                        group,
                        priority,
                        modifiers: 0,
                    });
                }
            }
        }
    }
}

fn dirty_byte_ranges(dirty: &[bool], offsets: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < dirty.len() {
        if !dirty[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < dirty.len() && dirty[i] {
            i += 1;
        }
        let end = i.saturating_sub(1);
        let start_byte = *offsets.get(start).unwrap_or(&0);
        let end_byte = *offsets.get(end + 1).unwrap_or(&start_byte);
        ranges.push((start_byte, end_byte));
    }
    ranges
}

fn build_line_map(old_lines: usize, edits: &[BufferEdit]) -> Vec<Option<usize>> {
    let mut map: Vec<Option<usize>> = (0..old_lines).map(Some).collect();
    for edit in edits {
        let start = edit.start_point.row;
        let old_end = edit.old_end_point.row;
        let new_end = edit.new_end_point.row;
        let delta = new_end as isize - old_end as isize;
        for entry in map.iter_mut() {
            let Some(line) = *entry else { continue };
            if line < start {
                continue;
            } else if line > old_end {
                let shifted = line as isize + delta;
                *entry = (shifted >= 0).then_some(shifted as usize);
            } else {
                *entry = None;
            }
        }
    }
    map
}

fn to_input_edit(edit: &BufferEdit) -> InputEdit {
    InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: to_point(edit.start_point),
        old_end_position: to_point(edit.old_end_point),
        new_end_position: to_point(edit.new_end_point),
    }
}

fn to_point(p: BufferPoint) -> Point {
    Point::new(p.row, p.column)
}

fn extract_panic_message(info: &(dyn std::any::Any + Send)) -> String {
    info.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Single-slot throttle: `f` runs at most once per `interval`. Seeded so the
/// first call always fires, even before `interval` has elapsed at process
/// start.
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
        let mut last = guard.lock().unwrap();
        if now.duration_since(*last) >= self.interval {
            *last = now;
            f();
        }
    }
}

static LOG_COMPLETE: RateLimiter = RateLimiter::new(Duration::from_secs(1));
static LOG_ERROR: RateLimiter = RateLimiter::new(Duration::from_secs(1));

fn log_completion(buffer_id: usize, version: u64, snapshot: &HighlightSnapshot) {
    LOG_COMPLETE.throttled(|| {
        let span_count: usize = snapshot.per_line.iter().map(|l| l.len()).sum();
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
