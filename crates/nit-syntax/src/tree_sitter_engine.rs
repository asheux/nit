use std::collections::HashMap;
use std::cmp;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use nit_core::{BufferEdit, BufferPoint};
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, Tree};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
use tracing::{error, warn};

use crate::engine::{HighlightRequest, SyntaxEngine};
use crate::highlight::{EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan, SyntaxStatus};
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
}

fn worker_loop(rx: Receiver<HighlightRequest>, out_tx: Sender<HighlightResult>) {
    let mut buffers: HashMap<usize, BufferState> = HashMap::new();
    let mut configs = build_configs();
    let mut query_configs = build_query_configs();
    let mut highlighter = Highlighter::new();

    loop {
        let first = match rx.recv() {
            Ok(job) => job,
            Err(_) => break,
        };
        let mut pending: HashMap<usize, HighlightRequest> = HashMap::new();
        pending.insert(first.buffer_id, first);
        while let Ok(job) = rx.try_recv() {
            pending.insert(job.buffer_id, job);
        }

        for (_, job) in pending.drain() {
            let start = Instant::now();
            let snapshot = match highlight_job(
                &mut buffers,
                &mut configs,
                &mut query_configs,
                &mut highlighter,
                &job,
            ) {
                Ok(mut snap) => {
                    snap.duration_ms = start.elapsed().as_millis();
                    log_highlight_complete(job.buffer_id, job.version, &snap);
                    snap
                }
                Err(err) => {
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
            };
            let _ = out_tx.send(HighlightResult {
                buffer_id: job.buffer_id,
                snapshot,
            });
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
            warn!("no highlight config for {:?}", language);
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
    });

    if buffer_state.language != language {
        buffer_state.language = language;
        buffer_state.tree = None;
        buffer_state.snapshot = None;
    }

    if let Some(lang) = LanguageRegistry::tree_sitter_language(language) {
        buffer_state.parser.set_language(lang)?;
    }

    let mut edited_old_tree = None;
    let mut tree = if job.full_reparse || buffer_state.tree.is_none() {
        buffer_state.parser.parse(job.text.as_bytes(), None)
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

    let Some(tree) = tree.take() else {
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
                    let end_line = cmp::min(end_line.saturating_add(1), new_line_count.saturating_sub(1));
                    for line in start_line..=end_line {
                        dirty[line] = true;
                    }
                }
            }

            for (idx, was_copied) in copied.iter().enumerate() {
                if !*was_copied {
                    dirty[idx] = true;
                }
            }

            if dirty.iter().any(|v| *v) {
                if let Some(query_cfg) = query_configs.get(&language) {
                    let mut spans = Vec::new();
                    let dirty_ranges = dirty_line_ranges(&dirty, &line_start_bytes);
                    collect_spans(
                        query_cfg,
                        &tree,
                        job.text.as_bytes(),
                        &dirty_ranges,
                        &mut spans,
                    );
                    let dirty_snapshot = HighlightSnapshot::from_spans(
                        job.buffer_id,
                        job.version,
                        job.language,
                        EngineKind::TreeSitter,
                        SyntaxStatus::Ok(EngineKind::TreeSitter),
                        &job.text,
                        spans,
                        job.max_spans_per_line,
                    );
                    for (idx, is_dirty) in dirty.iter().enumerate() {
                        if *is_dirty {
                            if let Some(line) = dirty_snapshot.per_line.get(idx) {
                                per_line[idx] = line.clone();
                            } else {
                                per_line[idx].clear();
                            }
                            if let Some(hash) = dirty_snapshot.line_hashes.get(idx) {
                                line_hashes[idx] = *hash;
                            }
                        }
                    }
                } else {
                    per_line.iter_mut().for_each(|line| line.clear());
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
            }
        } else {
            full_highlight(configs, config, highlighter, job)?
        }
    } else {
        full_highlight(configs, config, highlighter, job)?
    };

    buffer_state.tree = Some(tree);
    buffer_state.snapshot = Some(snapshot.clone());
    Ok(snapshot)
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

fn should_incremental_update(state: &BufferState, job: &HighlightRequest) -> bool {
    !job.full_reparse
        && !job.edits.is_empty()
        && state.snapshot.is_some()
        && state.tree.is_some()
        && LanguageRegistry::injections_query(job.language).is_empty()
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
) {
    let mut cursor = QueryCursor::new();
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
    let mut iter = highlighter.highlight(
        config,
        job.text.as_bytes(),
        None,
        |name| LanguageRegistry::from_injection_name(name).and_then(|id| configs.get(&id)),
    )?;
    while let Some(event) = iter.next() {
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
                    warn!(
                        "failed to build highlight config for {:?} with injections: {err}",
                        lang
                    );
                    match HighlightConfiguration::new(ts_lang, highlights, "", "") {
                        Ok(cfg) => cfg,
                        Err(err) => {
                            warn!(
                                "failed to build highlight config for {:?}: {err}",
                                lang
                            );
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
