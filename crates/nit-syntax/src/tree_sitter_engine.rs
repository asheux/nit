use std::cmp;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use nit_core::{BufferEdit, BufferPoint};
use tracing::{debug, error};
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, Tree};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::engine::{HighlightRequest, SyntaxEngine};
use crate::highlight::{
    EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan, LineSegment, SyntaxStatus,
};
use crate::registry::{LanguageId, LanguageRegistry};

pub struct TreeSitterEngine {
    sender: Sender<HighlightRequest>,
    receiver: Receiver<HighlightResult>,
    cache: HashMap<usize, HighlightSnapshot>,
}

struct HighlightResult {
    buffer_id: usize,
    snapshot: HighlightSnapshot,
}

impl TreeSitterEngine {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<HighlightRequest>();
        let (out_tx, out_rx) = mpsc::channel::<HighlightResult>();
        thread::spawn(move || worker_loop(rx, out_tx));
        Self {
            sender: tx,
            receiver: out_rx,
            cache: HashMap::new(),
        }
    }

    /// Pre-warm the worker for a language so grammar + parser are ready before content arrives.
    pub fn prewarm_language(&self, language: LanguageId) {
        let request = HighlightRequest {
            buffer_id: usize::MAX,
            version: 0,
            language,
            text: String::new(),
            edits: Vec::new(),
            full_reparse: true,
            max_spans_per_line: 0,
            viewport: None,
        };
        let _ = self.sender.send(request);
    }

    fn drain_results(&mut self) {
        while let Ok(result) = self.receiver.try_recv() {
            self.cache.insert(result.buffer_id, result.snapshot);
        }
    }
}

impl SyntaxEngine for TreeSitterEngine {
    fn detect_language(
        &self,
        path: Option<&std::path::Path>,
        first_line: Option<&str>,
        override_lang: Option<LanguageId>,
    ) -> LanguageId {
        LanguageRegistry::detect(path, first_line, override_lang)
    }

    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        let _ = self.sender.send(request);
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        self.drain_results();
        self.cache
            .get(&buffer_id)
            .filter(|snap| snap.version == version)
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

struct ProgressiveFillState {
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

fn worker_loop(rx: Receiver<HighlightRequest>, out_tx: Sender<HighlightResult>) {
    let mut buffers: HashMap<usize, BufferState> = HashMap::new();
    let mut configs = build_configs();
    let mut query_configs = build_query_configs();
    let mut highlighter = Highlighter::new();
    let mut progressive_fills: HashMap<usize, ProgressiveFillState> = HashMap::new();

    loop {
        // Use recv_timeout when progressive fills are pending, blocking recv otherwise
        let first = if progressive_fills.is_empty() {
            match rx.recv() {
                Ok(req) => Some(req),
                Err(_) => break,
            }
        } else {
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(req) => Some(req),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };

        if let Some(first) = first {
            // Batch drain
            let mut pending: HashMap<usize, HighlightRequest> = HashMap::new();
            pending.insert(first.buffer_id, first);
            while let Ok(job) = rx.try_recv() {
                pending.insert(job.buffer_id, job);
            }

            // Prioritize full_reparse requests (instant syntax on file open)
            let mut jobs: Vec<HighlightRequest> = pending.into_values().collect();
            jobs.sort_by_key(|j| u8::from(!j.full_reparse));

            for job in jobs {
                // Cancel progressive fill for this buffer
                progressive_fills.remove(&job.buffer_id);

                let start = Instant::now();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    highlight_job(
                        &mut buffers,
                        &mut configs,
                        &mut query_configs,
                        &mut highlighter,
                        &job,
                    )
                }));

                let snapshot = match result {
                    Ok(Ok(mut snap)) => {
                        snap.duration_ms = start.elapsed().as_millis();
                        log_highlight_complete(job.buffer_id, job.version, &snap);
                        snap
                    }
                    Ok(Err(err)) => {
                        log_highlight_error(job.buffer_id, job.version, &err);
                        let mut snap = HighlightSnapshot::plain(
                            job.buffer_id,
                            job.version,
                            job.language,
                            EngineKind::TreeSitter,
                            SyntaxStatus::Error(err.to_string()),
                            &job.text,
                        );
                        snap.duration_ms = start.elapsed().as_millis();
                        snap
                    }
                    Err(panic_info) => {
                        let msg = panic_message(&panic_info);
                        error!(
                            buffer_id = job.buffer_id,
                            version = job.version,
                            "syntax worker panic: {msg}"
                        );
                        // Clear buffer state to avoid cascading failures
                        buffers.remove(&job.buffer_id);
                        let mut snap = HighlightSnapshot::plain(
                            job.buffer_id,
                            job.version,
                            job.language,
                            EngineKind::TreeSitter,
                            SyntaxStatus::Error(format!("worker panic: {msg}")),
                            &job.text,
                        );
                        snap.duration_ms = start.elapsed().as_millis();
                        snap
                    }
                };

                // Set up progressive fill for viewport-scoped results
                if let Some((hl_start, hl_end)) = snapshot.highlighted_range {
                    let total = snapshot.per_line.len();
                    if hl_end + 1 < total || hl_start > 0 {
                        progressive_fills.insert(
                            job.buffer_id,
                            ProgressiveFillState {
                                buffer_id: job.buffer_id,
                                version: job.version,
                                language: job.language,
                                text: job.text.clone(),
                                line_start_bytes: snapshot.line_start_bytes.clone(),
                                next_below: if hl_end + 1 < total {
                                    Some(hl_end + 1)
                                } else {
                                    None
                                },
                                next_above: if hl_start > 0 { Some(hl_start) } else { None },
                                chunk_size: 500,
                                total_lines: total,
                                max_spans_per_line: job.max_spans_per_line,
                            },
                        );
                    }
                }

                let _ = out_tx.send(HighlightResult {
                    buffer_id: job.buffer_id,
                    snapshot,
                });
            }
        }

        // Process one progressive fill chunk per idle cycle
        let pf_ids: Vec<usize> = progressive_fills.keys().copied().collect();
        let mut done_ids = Vec::new();
        for id in pf_ids {
            let pf = progressive_fills.get_mut(&id).unwrap();
            if process_progressive_chunk(pf, &mut buffers, &query_configs, &out_tx) {
                done_ids.push(id);
            }
        }
        for id in done_ids {
            progressive_fills.remove(&id);
        }
    }
}

fn highlight_job(
    buffers: &mut HashMap<usize, BufferState>,
    configs: &mut HashMap<LanguageId, HighlightConfiguration>,
    query_configs: &mut HashMap<LanguageId, QueryConfig>,
    highlighter: &mut Highlighter,
    job: &HighlightRequest,
) -> anyhow::Result<HighlightSnapshot> {
    let language = job.language;
    let config = match configs.get(&language) {
        Some(cfg) => cfg,
        None => {
            debug!("no highlight config for {:?}", language);
            return Ok(HighlightSnapshot::plain(
                job.buffer_id,
                job.version,
                job.language,
                EngineKind::TreeSitter,
                SyntaxStatus::Error("no highlight config".into()),
                &job.text,
            ));
        }
    };

    let buffer_state = buffers.entry(job.buffer_id).or_insert_with(|| BufferState {
        language,
        parser: Parser::new(),
        tree: None,
        snapshot: None,
        cursor: QueryCursor::new(),
    });

    // Bug 4 fix: invalidate cache on language change
    if buffer_state.language != language {
        buffer_state.language = language;
        buffer_state.tree = None;
        buffer_state.snapshot = None;
    }

    if let Some(lang) = LanguageRegistry::tree_sitter_language(language) {
        buffer_state.parser.set_language(lang)?;
    }

    let mut edited_old_tree = None;
    let tree = if job.full_reparse || buffer_state.tree.is_none() {
        buffer_state.parser.parse(job.text.as_bytes(), None)
    } else if job.edits.is_empty() {
        // Scroll-only: reuse existing tree without reparsing
        buffer_state.tree.take()
    } else {
        let mut existing = buffer_state.tree.take().unwrap();
        for edit in &job.edits {
            existing.edit(&to_input_edit(edit));
        }
        edited_old_tree = Some(existing.clone());
        buffer_state
            .parser
            .parse(job.text.as_bytes(), Some(&existing))
            .or(Some(existing))
    };

    let Some(tree) = tree else {
        return Ok(HighlightSnapshot::plain(
            job.buffer_id,
            job.version,
            job.language,
            EngineKind::TreeSitter,
            SyntaxStatus::Error("parse failed".into()),
            &job.text,
        ));
    };

    let snapshot = if should_incremental_update(buffer_state, job) {
        if let Some(old_snapshot) = buffer_state.snapshot.as_ref() {
            let line_start_bytes = compute_line_starts(&job.text);
            let new_line_count = line_start_bytes.len().saturating_sub(1);
            let mut per_line = vec![Vec::new(); new_line_count];
            let mut line_hashes = vec![0u64; new_line_count];
            let mut copied = vec![false; new_line_count];
            let line_map = build_line_map(old_snapshot.per_line.len(), &job.edits);
            for (old_idx, new_idx) in line_map.into_iter().enumerate() {
                if let Some(new_idx) = new_idx {
                    if new_idx < new_line_count {
                        per_line[new_idx] = old_snapshot.per_line[old_idx].clone();
                        if let Some(hash) = old_snapshot.line_hashes.get(old_idx) {
                            line_hashes[new_idx] = *hash;
                        }
                        copied[new_idx] = true;
                    }
                }
            }

            let mut dirty = vec![false; new_line_count];
            if let Some(ref old_tree) = edited_old_tree {
                let ranges = old_tree.changed_ranges(&tree);
                for range in ranges {
                    if range.end_byte == 0 || new_line_count == 0 {
                        continue;
                    }
                    let start_line = find_line(&line_start_bytes, range.start_byte);
                    let end_byte = range.end_byte.saturating_sub(1);
                    let end_line = find_line(&line_start_bytes, end_byte);
                    let start_line = start_line.saturating_sub(1);
                    let end_line =
                        cmp::min(end_line.saturating_add(1), new_line_count.saturating_sub(1));
                    let end = end_line.saturating_add(1).min(dirty.len());
                    for slot in dirty.iter_mut().take(end).skip(start_line) {
                        *slot = true;
                    }
                }
            }

            for (idx, was_copied) in copied.iter().enumerate() {
                if !*was_copied {
                    dirty[idx] = true;
                }
            }

            if dirty.iter().any(|v| *v) {
                for (idx, is_dirty) in dirty.iter().enumerate() {
                    if *is_dirty {
                        per_line[idx].clear();
                    }
                }

                if let Some(query_cfg) = query_configs.get(&language) {
                    let mut spans = Vec::new();
                    let dirty_ranges = dirty_line_ranges(&dirty, &line_start_bytes);
                    collect_spans(
                        query_cfg,
                        &tree,
                        job.text.as_bytes(),
                        &dirty_ranges,
                        &mut spans,
                        &mut buffer_state.cursor,
                    );
                    apply_spans_to_dirty_lines(
                        &mut spans,
                        &dirty,
                        &line_start_bytes,
                        &mut per_line,
                        job.max_spans_per_line,
                    );
                }

                let bytes = job.text.as_bytes();
                for (idx, is_dirty) in dirty.iter().enumerate() {
                    if !*is_dirty {
                        continue;
                    }
                    if idx + 1 >= line_start_bytes.len() {
                        continue;
                    }
                    let start = line_start_bytes[idx];
                    let end = line_start_bytes[idx + 1];
                    line_hashes[idx] = crate::highlight::hash_line_bytes(&bytes[start..end]);
                }
            }

            HighlightSnapshot {
                buffer_id: job.buffer_id,
                version: job.version,
                language: job.language,
                engine: EngineKind::TreeSitter,
                status: SyntaxStatus::Ok(EngineKind::TreeSitter),
                duration_ms: 0,
                line_start_bytes,
                line_hashes,
                per_line,
                highlighted_range: None,
            }
        } else {
            full_highlight(configs, config, highlighter, job)?
        }
    } else if job.viewport.is_some() {
        // Viewport-scoped highlighting for large files
        viewport_highlight(query_configs, &tree, job, &mut buffer_state.cursor)?
    } else {
        full_highlight(configs, config, highlighter, job)?
    };

    buffer_state.tree = Some(tree);
    buffer_state.snapshot = Some(snapshot.clone());
    Ok(snapshot)
}

fn viewport_highlight(
    query_configs: &HashMap<LanguageId, QueryConfig>,
    tree: &Tree,
    job: &HighlightRequest,
    cursor: &mut QueryCursor,
) -> anyhow::Result<HighlightSnapshot> {
    let vp = job.viewport.as_ref().unwrap();
    let line_start_bytes = compute_line_starts(&job.text);
    let total_lines = line_start_bytes.len().saturating_sub(1);

    let buffer_zone = 100;
    let start_line = vp.first_line.saturating_sub(buffer_zone);
    let end_line = (vp.last_line + buffer_zone).min(total_lines.saturating_sub(1));

    let start_byte = line_start_bytes[start_line];
    let end_byte = line_start_bytes
        .get(end_line + 1)
        .copied()
        .unwrap_or(job.text.len());

    let query_cfg = match query_configs.get(&job.language) {
        Some(cfg) => cfg,
        None => {
            return Ok(HighlightSnapshot::plain(
                job.buffer_id,
                job.version,
                job.language,
                EngineKind::TreeSitter,
                SyntaxStatus::Error("no query config".into()),
                &job.text,
            ));
        }
    };

    let mut spans = Vec::new();
    collect_spans(
        query_cfg,
        tree,
        job.text.as_bytes(),
        &[(start_byte, end_byte)],
        &mut spans,
        cursor,
    );

    // Sort spans - skip sort if already ordered (optimization)
    let needs_sort = spans.windows(2).any(|pair| {
        let a = &pair[0];
        let b = &pair[1];
        match a.start_byte.cmp(&b.start_byte) {
            cmp::Ordering::Greater => true,
            cmp::Ordering::Equal => a.priority < b.priority,
            cmp::Ordering::Less => false,
        }
    });
    if needs_sort {
        spans.sort_by(|a, b| match a.start_byte.cmp(&b.start_byte) {
            cmp::Ordering::Equal => b.priority.cmp(&a.priority),
            other => other,
        });
    }

    // Build per_line only for highlighted range; other lines get empty Vec
    let mut per_line = vec![Vec::new(); total_lines];
    for span in &spans {
        if span.end_byte <= span.start_byte {
            continue;
        }
        let sl = find_line(&line_start_bytes, span.start_byte);
        let el = find_line(&line_start_bytes, span.end_byte);
        for line in sl..=el {
            if line + 1 >= line_start_bytes.len() {
                continue;
            }
            if job.max_spans_per_line > 0 && per_line[line].len() >= job.max_spans_per_line {
                continue;
            }
            let line_start = line_start_bytes[line];
            let line_end = line_start_bytes[line + 1];
            let seg_start = span.start_byte.max(line_start) - line_start;
            let seg_end = span.end_byte.min(line_end) - line_start;
            if seg_start < seg_end {
                per_line[line].push(LineSegment {
                    start: seg_start,
                    end: seg_end,
                    group: span.group,
                });
            }
        }
    }

    // Compute line hashes only for highlighted range (sentinel 0 for others)
    let bytes = job.text.as_bytes();
    let mut line_hashes = vec![0u64; total_lines];
    for idx in start_line..=end_line.min(total_lines.saturating_sub(1)) {
        if idx + 1 < line_start_bytes.len() {
            let s = line_start_bytes[idx];
            let e = line_start_bytes[idx + 1];
            line_hashes[idx] = crate::highlight::hash_line_bytes(&bytes[s..e]);
        }
    }

    Ok(HighlightSnapshot {
        buffer_id: job.buffer_id,
        version: job.version,
        language: job.language,
        engine: EngineKind::TreeSitter,
        status: SyntaxStatus::Ok(EngineKind::TreeSitter),
        duration_ms: 0,
        line_start_bytes,
        line_hashes,
        per_line,
        highlighted_range: Some((start_line, end_line)),
    })
}

fn process_progressive_chunk(
    pf: &mut ProgressiveFillState,
    buffers: &mut HashMap<usize, BufferState>,
    query_configs: &HashMap<LanguageId, QueryConfig>,
    out_tx: &Sender<HighlightResult>,
) -> bool {
    // Determine chunk range: fill below first, then above
    let (start_line, end_line) = if let Some(next) = pf.next_below {
        let end = (next + pf.chunk_size).min(pf.total_lines);
        let end_line = end.saturating_sub(1);
        pf.next_below = if end < pf.total_lines {
            Some(end)
        } else {
            None
        };
        (next, end_line)
    } else if let Some(above_end) = pf.next_above {
        let start = above_end.saturating_sub(pf.chunk_size);
        let end_line = above_end.saturating_sub(1);
        pf.next_above = if start > 0 { Some(start) } else { None };
        (start, end_line)
    } else {
        return true; // done
    };

    let query_cfg = match query_configs.get(&pf.language) {
        Some(cfg) => cfg,
        None => return true,
    };

    let buffer_state = match buffers.get_mut(&pf.buffer_id) {
        Some(bs) => bs,
        None => return true,
    };

    let tree = match buffer_state.tree.as_ref() {
        Some(t) => t,
        None => return true,
    };

    let snapshot = match buffer_state.snapshot.as_mut() {
        Some(s) => s,
        None => return true,
    };

    if snapshot.version != pf.version {
        return true; // version mismatch, cancel
    }

    let start_byte = pf.line_start_bytes[start_line];
    let end_byte = pf
        .line_start_bytes
        .get(end_line + 1)
        .copied()
        .unwrap_or(pf.text.len());

    let mut spans = Vec::new();
    collect_spans(
        query_cfg,
        tree,
        pf.text.as_bytes(),
        &[(start_byte, end_byte)],
        &mut spans,
        &mut buffer_state.cursor,
    );

    spans.sort_by(|a, b| match a.start_byte.cmp(&b.start_byte) {
        cmp::Ordering::Equal => b.priority.cmp(&a.priority),
        other => other,
    });

    for span in &spans {
        if span.end_byte <= span.start_byte {
            continue;
        }
        let sl = find_line(&pf.line_start_bytes, span.start_byte);
        let el = find_line(&pf.line_start_bytes, span.end_byte);
        for line in sl..=el {
            if line < start_line || line > end_line {
                continue;
            }
            if line + 1 >= pf.line_start_bytes.len() || line >= snapshot.per_line.len() {
                continue;
            }
            if pf.max_spans_per_line > 0 && snapshot.per_line[line].len() >= pf.max_spans_per_line {
                continue;
            }
            let line_start = pf.line_start_bytes[line];
            let line_end = pf.line_start_bytes[line + 1];
            let seg_start = span.start_byte.max(line_start) - line_start;
            let seg_end = span.end_byte.min(line_end) - line_start;
            if seg_start < seg_end {
                snapshot.per_line[line].push(LineSegment {
                    start: seg_start,
                    end: seg_end,
                    group: span.group,
                });
            }
        }
    }

    // Update line hashes for chunk
    let bytes = pf.text.as_bytes();
    for idx in start_line..=end_line {
        if idx + 1 < pf.line_start_bytes.len() && idx < snapshot.line_hashes.len() {
            let s = pf.line_start_bytes[idx];
            let e = pf.line_start_bytes[idx + 1];
            snapshot.line_hashes[idx] = crate::highlight::hash_line_bytes(&bytes[s..e]);
        }
    }

    // Expand highlighted_range
    let (old_start, old_end) = snapshot
        .highlighted_range
        .unwrap_or((0, pf.total_lines.saturating_sub(1)));
    let new_start = old_start.min(start_line);
    let new_end = old_end.max(end_line);
    if new_start == 0 && new_end >= pf.total_lines.saturating_sub(1) {
        snapshot.highlighted_range = None; // fully covered
    } else {
        snapshot.highlighted_range = Some((new_start, new_end));
    }

    let _ = out_tx.send(HighlightResult {
        buffer_id: pf.buffer_id,
        snapshot: snapshot.clone(),
    });

    pf.next_below.is_none() && pf.next_above.is_none()
}

fn panic_message(info: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = info.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = info.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn apply_spans_to_dirty_lines(
    spans: &mut [HighlightSpan],
    dirty: &[bool],
    line_starts: &[usize],
    per_line: &mut [Vec<LineSegment>],
    max_spans_per_line: usize,
) {
    spans.sort_by(|a, b| match a.start_byte.cmp(&b.start_byte) {
        cmp::Ordering::Equal => b.priority.cmp(&a.priority),
        other => other,
    });

    for span in spans.iter() {
        if span.end_byte <= span.start_byte {
            continue;
        }
        let start_line = find_line(line_starts, span.start_byte);
        let end_line = find_line(line_starts, span.end_byte);
        for line in start_line..=end_line {
            if line + 1 >= line_starts.len() || line >= dirty.len() || !dirty[line] {
                continue;
            }
            if max_spans_per_line > 0 && per_line[line].len() >= max_spans_per_line {
                continue;
            }
            let line_start = line_starts[line];
            let line_end = line_starts[line + 1];
            let seg_start = span.start_byte.max(line_start) - line_start;
            let seg_end = span.end_byte.min(line_end) - line_start;
            if seg_start < seg_end {
                per_line[line].push(LineSegment {
                    start: seg_start,
                    end: seg_end,
                    group: span.group,
                });
            }
        }
    }
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

fn to_point(point: BufferPoint) -> Point {
    Point::new(point.row, point.column)
}

// Bug 1 fix: allow incremental updates for injection languages.
// For injection languages, the outer language spans are re-queried correctly via
// collect_spans on dirty ranges. Injection-specific highlighting in dirty regions
// may be temporarily absent until the next full reparse, but this is a significant
// perf improvement over full re-highlighting every keystroke.
fn should_incremental_update(state: &BufferState, job: &HighlightRequest) -> bool {
    !job.full_reparse && !job.edits.is_empty() && state.snapshot.is_some() && state.tree.is_some()
}

struct QueryConfig {
    query: Query,
    capture_groups: Vec<Option<HighlightGroup>>,
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
            for capture in m.captures {
                let group = cfg
                    .capture_groups
                    .get(capture.index as usize)
                    .and_then(|g| *g);
                let Some(group) = group else {
                    continue;
                };
                if group == HighlightGroup::Normal {
                    continue;
                }
                let node = capture.node;
                let mut start_byte = node.start_byte();
                let mut end_byte = node.end_byte();
                if end_byte <= start || start_byte >= end {
                    continue;
                }
                if start_byte < start {
                    start_byte = start;
                }
                if end_byte > end {
                    end_byte = end;
                }
                if end_byte > start_byte {
                    spans.push(HighlightSpan {
                        start_byte,
                        end_byte,
                        group,
                        priority,
                        modifiers: 0,
                    });
                }
            }
        }
    }
}

fn dirty_line_ranges(dirty: &[bool], line_starts: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut idx = 0;
    while idx < dirty.len() {
        if !dirty[idx] {
            idx += 1;
            continue;
        }
        let start_line = idx;
        while idx < dirty.len() && dirty[idx] {
            idx += 1;
        }
        let end_line = idx.saturating_sub(1);
        let start_byte = *line_starts.get(start_line).unwrap_or(&0);
        let end_byte = *line_starts.get(end_line + 1).unwrap_or(&start_byte);
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
            let Some(line) = *entry else {
                continue;
            };
            if line < start {
                continue;
            } else if line > old_end {
                let shifted = line as isize + delta;
                *entry = if shifted >= 0 {
                    Some(shifted as usize)
                } else {
                    None
                };
            } else {
                *entry = None;
            }
        }
    }
    map
}

fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(idx + 1);
        }
    }
    if *starts.last().unwrap_or(&0) != text.len() {
        starts.push(text.len());
    }
    starts
}

fn find_line(starts: &[usize], byte: usize) -> usize {
    let idx = starts.partition_point(|&s| s <= byte);
    idx.saturating_sub(1)
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
            HighlightEvent::HighlightStart(s) => {
                let group = CAPTURE_GROUPS
                    .get(s.0)
                    .copied()
                    .unwrap_or(HighlightGroup::Normal);
                stack.push(group);
            }
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

fn build_query_configs() -> HashMap<LanguageId, QueryConfig> {
    let mut configs = HashMap::new();
    let mut group_map = HashMap::new();
    for (idx, name) in CAPTURE_NAMES.iter().enumerate() {
        if let Some(group) = CAPTURE_GROUPS.get(idx) {
            group_map.insert(*name, *group);
        }
    }
    for lang in LanguageId::ALL {
        let Some(ts_lang) = LanguageRegistry::tree_sitter_language(lang) else {
            continue;
        };
        let Some(query_src) = LanguageRegistry::highlights_query(lang) else {
            continue;
        };
        let Ok(query) = Query::new(ts_lang, query_src) else {
            continue;
        };
        let capture_groups = query
            .capture_names()
            .iter()
            .map(|name| {
                if let Some(group) = group_map.get(name.as_str()) {
                    return Some(*group);
                }
                let base = name.split('.').next().unwrap_or(name);
                group_map.get(base).copied()
            })
            .collect::<Vec<_>>();
        configs.insert(
            lang,
            QueryConfig {
                query,
                capture_groups,
            },
        );
    }
    configs
}

fn log_highlight_complete(buffer_id: usize, version: u64, snapshot: &HighlightSnapshot) {
    log_rate_limited(&HIGHLIGHT_COMPLETE_LOG, Duration::from_secs(1), || {
        let span_count: usize = snapshot.per_line.iter().map(|line| line.len()).sum();
        tracing::debug!(
            buffer_id,
            version,
            span_count,
            duration_ms = snapshot.duration_ms,
            "syntax highlight complete"
        );
    });
}

fn log_highlight_error(buffer_id: usize, version: u64, err: &anyhow::Error) {
    log_rate_limited(&HIGHLIGHT_ERROR_LOG, Duration::from_secs(1), || {
        error!(
            buffer_id,
            version,
            error = %err,
            "syntax highlight error"
        );
    });
}

fn log_rate_limited(lock: &'static OnceLock<Mutex<Instant>>, interval: Duration, f: impl FnOnce()) {
    let now = Instant::now();
    let guard = lock.get_or_init(|| Mutex::new(now - interval));
    let mut last = guard.lock().unwrap();
    if now.duration_since(*last) >= interval {
        *last = now;
        f();
    }
}

static HIGHLIGHT_COMPLETE_LOG: OnceLock<Mutex<Instant>> = OnceLock::new();
static HIGHLIGHT_ERROR_LOG: OnceLock<Mutex<Instant>> = OnceLock::new();

const CAPTURE_NAMES: &[&str] = &[
    "comment",
    "comment.documentation",
    "string",
    "string.special",
    "character",
    "number",
    "boolean",
    "keyword",
    "keyword.control",
    "keyword.operator",
    "type",
    "type.builtin",
    "function",
    "method",
    "macro",
    "attribute",
    "namespace",
    "variable",
    "parameter",
    "property",
    "constant",
    "operator",
    "punctuation",
    "tag",
    "heading",
    "emphasis",
    "link",
    "error",
    "warning",
    "constant.builtin",
    "function.macro",
    "function.method",
    "variable.parameter",
    "variable.builtin",
    "punctuation.bracket",
    "punctuation.delimiter",
    "constructor",
    "label",
    "escape",
];

const CAPTURE_GROUPS: &[HighlightGroup] = &[
    HighlightGroup::Comment,
    HighlightGroup::DocComment,
    HighlightGroup::String,
    HighlightGroup::String,
    HighlightGroup::Char,
    HighlightGroup::Number,
    HighlightGroup::Boolean,
    HighlightGroup::Keyword,
    HighlightGroup::KeywordControl,
    HighlightGroup::KeywordOperator,
    HighlightGroup::Type,
    HighlightGroup::TypeBuiltin,
    HighlightGroup::Function,
    HighlightGroup::Method,
    HighlightGroup::Macro,
    HighlightGroup::Attribute,
    HighlightGroup::Namespace,
    HighlightGroup::Variable,
    HighlightGroup::Parameter,
    HighlightGroup::Property,
    HighlightGroup::Constant,
    HighlightGroup::Operator,
    HighlightGroup::Punctuation,
    HighlightGroup::Tag,
    HighlightGroup::Heading,
    HighlightGroup::Emphasis,
    HighlightGroup::Link,
    HighlightGroup::Error,
    HighlightGroup::Warning,
    HighlightGroup::Number,
    HighlightGroup::Macro,
    HighlightGroup::Method,
    HighlightGroup::Parameter,
    HighlightGroup::Variable,
    HighlightGroup::Punctuation,
    HighlightGroup::Punctuation,
    HighlightGroup::Type,
    HighlightGroup::KeywordControl,
    HighlightGroup::String,
];

fn build_configs() -> HashMap<LanguageId, HighlightConfiguration> {
    let mut configs = HashMap::new();
    for lang in LanguageId::ALL {
        if let Some(ts_lang) = LanguageRegistry::tree_sitter_language(lang) {
            let highlights = LanguageRegistry::highlights_query(lang).unwrap_or("");
            let injections = LanguageRegistry::injections_query(lang);
            let mut config = match HighlightConfiguration::new(ts_lang, highlights, injections, "")
            {
                Ok(cfg) => cfg,
                Err(err) => {
                    debug!(
                        "failed to build highlight config for {:?} with injections: {err}",
                        lang
                    );
                    match HighlightConfiguration::new(ts_lang, highlights, "", "") {
                        Ok(cfg) => cfg,
                        Err(err) => {
                            debug!("failed to build highlight config for {:?}: {err}", lang);
                            continue;
                        }
                    }
                }
            };
            config.configure(CAPTURE_NAMES);
            configs.insert(lang, config);
        }
    }
    configs
}
