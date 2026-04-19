use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nit_core::{Buffer, BufferEdit, HighlightConfig, HighlightEngine};
use nit_syntax::{
    Debouncer, HighlightRequest, HighlightSnapshot, LanguageId, SyntaxEngine, SyntaxManager,
    ViewportRange,
};

#[derive(Debug, Clone)]
struct PendingSyntax {
    version: u64,
    language: LanguageId,
    edits: Vec<BufferEdit>,
    full_reparse: bool,
}

#[derive(Clone, Debug)]
struct VersionedEdit {
    version: u64,
    edit: BufferEdit,
}

pub struct RenderSnapshot<'a> {
    pub snapshot: Option<&'a HighlightSnapshot>,
    pub line_map: Option<&'a [Option<usize>]>,
}

struct CachedLineHashes {
    version: u64,
    hashes: Arc<[u64]>,
}

struct RenderCache {
    buffer_version: u64,
    snapshot_version: u64,
    line_map: Option<Vec<Option<usize>>>,
}

/// Per-buffer state for viewport-scoped highlighting.
#[derive(Default)]
struct BufferSyntaxState {
    /// Last viewport sent in a scroll-triggered rehighlight request.
    last_sent_viewport: Option<ViewportRange>,
    /// Last viewport checked (for debounce tracking).
    last_checked_viewport: Option<ViewportRange>,
    /// Timestamp of last scroll-triggered rehighlight send.
    last_scroll_send: Option<Instant>,
}

pub struct SyntaxRuntime {
    manager: SyntaxManager,
    debouncers: HashMap<usize, Debouncer>,
    pending: HashMap<usize, PendingSyntax>,
    snapshots: HashMap<usize, HighlightSnapshot>,
    last_sent: HashMap<usize, u64>,
    edits_since_snapshot: HashMap<usize, Vec<VersionedEdit>>,
    full_reparse_pending: HashMap<usize, bool>,
    line_hash_cache: HashMap<usize, CachedLineHashes>,
    render_cache: HashMap<usize, RenderCache>,
    scroll_debouncers: HashMap<usize, Debouncer>,
    buffer_syntax_states: HashMap<usize, BufferSyntaxState>,
}

const INITIAL_HIGHLIGHT_WAIT_MS: u64 = 1000;
const SCROLL_DEBOUNCE_MS: u64 = 20;
// Cap line-map computation on buffers above these thresholds; big files keep
// rendering with the latest snapshot and skip the diff-and-map step.
const LARGE_MAP_BYTES: usize = 600_000;
const LARGE_MAP_LINES: usize = 15_000;

impl SyntaxRuntime {
    pub fn new(config: HighlightConfig) -> Self {
        let manager = SyntaxManager::new(config_to_syntax(config));
        Self {
            manager,
            debouncers: HashMap::new(),
            pending: HashMap::new(),
            snapshots: HashMap::new(),
            last_sent: HashMap::new(),
            edits_since_snapshot: HashMap::new(),
            full_reparse_pending: HashMap::new(),
            line_hash_cache: HashMap::new(),
            render_cache: HashMap::new(),
            scroll_debouncers: HashMap::new(),
            buffer_syntax_states: HashMap::new(),
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
            self.last_sent.clear();
            self.edits_since_snapshot.clear();
            self.full_reparse_pending.clear();
            self.line_hash_cache.clear();
            self.render_cache.clear();
            self.scroll_debouncers.clear();
            self.buffer_syntax_states.clear();
        } else {
            self.line_hash_cache.clear();
            self.render_cache.clear();
        }
    }

    /// Pre-warm the syntax engine for a file path before content is loaded.
    pub fn prewarm_for_path(&self, path: &Path) {
        if !self.manager.config().enabled {
            return;
        }
        let language = self.manager.detect_language(Some(path), None, None);
        if language != LanguageId::PlainText {
            self.manager.prewarm_language(language);
        }
    }

    pub fn prime_buffer(&mut self, buffer_id: usize, buffer: &Buffer, warmup: bool) {
        if !self.manager.config().enabled {
            return;
        }
        let max_spans_per_line = adaptive_max_spans_per_line(
            self.manager.config().max_spans_per_line,
            buffer.bytes_len(),
        );
        let first_line = buffer.first_line();
        let language = self.manager.detect_language(
            buffer.path().map(|p| p.as_path()),
            first_line.as_deref(),
            None,
        );

        // For large files, include viewport for scoped highlighting
        let viewport = if buffer.bytes_len() > self.manager.config().max_file_bytes {
            Some(viewport_from_buffer(buffer))
        } else {
            None
        };

        // Send request immediately - skip debounce for initial file load (instant syntax)
        let request = HighlightRequest {
            buffer_id,
            version: buffer.version(),
            language,
            text: buffer.content_as_string(),
            edits: Vec::new(),
            full_reparse: true,
            max_spans_per_line,
            viewport,
        };
        self.last_sent.insert(buffer_id, request.version);
        self.manager.schedule_rehighlight(request);
        if let Some(debouncer) = self.debouncers.get_mut(&buffer_id) {
            debouncer.clear();
        }
        self.pending.remove(&buffer_id);
        self.render_cache.remove(&buffer_id);
        self.line_hash_cache.remove(&buffer_id);

        // Initialize viewport tracking for large files
        if buffer.bytes_len() > self.manager.config().max_file_bytes {
            let vp = viewport_from_buffer(buffer);
            self.buffer_syntax_states.insert(
                buffer_id,
                BufferSyntaxState {
                    last_sent_viewport: Some(vp.clone()),
                    last_checked_viewport: Some(vp),
                    last_scroll_send: Some(Instant::now()),
                },
            );
        }

        if warmup {
            let deadline = Instant::now() + Duration::from_millis(INITIAL_HIGHLIGHT_WAIT_MS);
            loop {
                if let Some(snapshot) = self.manager.try_get_highlights(buffer_id, buffer.version())
                {
                    self.snapshots.insert(buffer_id, snapshot);
                    self.trim_edits_since_snapshot(buffer_id);
                    self.render_cache.remove(&buffer_id);
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    pub fn note_buffer_change(&mut self, buffer_id: usize, buffer: &mut Buffer) {
        let edits = buffer.take_pending_edits();
        let full_reparse = buffer.take_full_reparse();
        if edits.is_empty() && !full_reparse {
            return;
        }
        if !edits.is_empty() {
            let current_version = buffer.version();
            let start_version = current_version
                .saturating_sub(edits.len() as u64)
                .saturating_add(1);
            let entry = self.edits_since_snapshot.entry(buffer_id).or_default();
            for (idx, edit) in edits.iter().enumerate() {
                entry.push(VersionedEdit {
                    version: start_version + idx as u64,
                    edit: edit.clone(),
                });
            }
        }
        if full_reparse {
            self.edits_since_snapshot.remove(&buffer_id);
            self.full_reparse_pending.insert(buffer_id, true);
        }
        self.render_cache.remove(&buffer_id);
        self.line_hash_cache.remove(&buffer_id);
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
        let debounce_ms =
            adaptive_debounce_ms(self.manager.config().debounce_ms, buffer.bytes_len());
        let debouncer = self
            .debouncers
            .entry(buffer_id)
            .or_insert_with(|| Debouncer::new(debounce_ms));
        if debouncer.delay() != Duration::from_millis(debounce_ms) {
            *debouncer = Debouncer::new(debounce_ms);
        }
        debouncer.mark();
    }

    pub fn tick(&mut self, buffer_id: usize, buffer: &Buffer) {
        if !self.manager.config().enabled {
            self.pending.remove(&buffer_id);
            if let Some(debouncer) = self.debouncers.get_mut(&buffer_id) {
                debouncer.clear();
            }
            self.last_sent.remove(&buffer_id);
            self.edits_since_snapshot.remove(&buffer_id);
            self.full_reparse_pending.remove(&buffer_id);
            self.scroll_debouncers.remove(&buffer_id);
            self.buffer_syntax_states.remove(&buffer_id);
            return;
        }
        self.tick_edits(buffer_id, buffer);
        self.tick_scroll(buffer_id, buffer);
    }

    fn tick_edits(&mut self, buffer_id: usize, buffer: &Buffer) {
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
        let max_spans_per_line = adaptive_max_spans_per_line(
            self.manager.config().max_spans_per_line,
            buffer.bytes_len(),
        );

        // Include viewport for large files
        let viewport = if buffer.bytes_len() > self.manager.config().max_file_bytes {
            Some(viewport_from_buffer(buffer))
        } else {
            None
        };

        let request = HighlightRequest {
            buffer_id,
            version: pending.version,
            language: pending.language,
            text,
            edits: pending.edits,
            full_reparse: pending.full_reparse,
            max_spans_per_line,
            viewport,
        };
        self.last_sent.insert(buffer_id, request.version);
        log_rate_limited(&HIGHLIGHT_SCHEDULE_LOG, Duration::from_secs(1), || {
            tracing::debug!(
                buffer_id,
                version = request.version,
                edits = request.edits.len(),
                full_reparse = request.full_reparse,
                "schedule syntax highlight"
            );
        });
        self.manager.schedule_rehighlight(request);
        debouncer.clear();
    }

    fn tick_scroll(&mut self, buffer_id: usize, buffer: &Buffer) {
        if buffer.bytes_len() <= self.manager.config().max_file_bytes {
            return;
        }

        let current_vp = viewport_from_buffer(buffer);
        let state = self.buffer_syntax_states.entry(buffer_id).or_default();

        // Check if viewport changed since last check
        let viewport_changed = state.last_checked_viewport.as_ref() != Some(&current_vp);
        if viewport_changed {
            state.last_checked_viewport = Some(current_vp.clone());
        }

        // Check if rehighlight is needed
        let highlighted_range = self
            .snapshots
            .get(&buffer_id)
            .and_then(|s| s.highlighted_range);
        let needs = viewport_needs_rehighlight(&current_vp, highlighted_range);

        if !needs {
            return;
        }

        // Debounce scroll events
        let debouncer = self
            .scroll_debouncers
            .entry(buffer_id)
            .or_insert_with(|| Debouncer::new(SCROLL_DEBOUNCE_MS));

        if viewport_changed {
            debouncer.mark();
        }

        if !debouncer.ready() {
            return;
        }

        // Send scroll-triggered rehighlight
        let language = self.manager.detect_language(
            buffer.path().map(|p| p.as_path()),
            buffer.first_line().as_deref(),
            None,
        );
        let max_spans = adaptive_max_spans_per_line(
            self.manager.config().max_spans_per_line,
            buffer.bytes_len(),
        );
        let request = HighlightRequest {
            buffer_id,
            version: buffer.version(),
            language,
            text: buffer.content_as_string(),
            edits: Vec::new(),
            full_reparse: false,
            max_spans_per_line: max_spans,
            viewport: Some(current_vp.clone()),
        };
        self.last_sent.insert(buffer_id, request.version);
        self.manager.schedule_rehighlight(request);
        state.last_sent_viewport = Some(current_vp);
        state.last_scroll_send = Some(Instant::now());
        debouncer.clear();
    }

    pub fn poll_results(&mut self, buffer_id: usize, version: u64) {
        if let Some(snapshot) = self.manager.try_get_highlights(buffer_id, version) {
            self.snapshots.insert(buffer_id, snapshot);
            self.trim_edits_since_snapshot(buffer_id);
            self.render_cache.remove(&buffer_id);
            return;
        }
        if let Some(last_sent) = self.last_sent.get(&buffer_id).copied() {
            if last_sent != version {
                if let Some(snapshot) = self.manager.try_get_highlights(buffer_id, last_sent) {
                    self.snapshots.insert(buffer_id, snapshot);
                    self.trim_edits_since_snapshot(buffer_id);
                    self.render_cache.remove(&buffer_id);
                }
            }
        }
    }

    pub fn snapshot_for(&self, buffer_id: usize, version: u64) -> Option<&HighlightSnapshot> {
        self.snapshots
            .get(&buffer_id)
            .filter(|snap| snap.version == version)
    }

    pub fn latest_snapshot_for(&self, buffer_id: usize) -> Option<&HighlightSnapshot> {
        self.snapshots.get(&buffer_id)
    }

    pub fn render_snapshot_for(&mut self, buffer_id: usize, buffer: &Buffer) -> RenderSnapshot<'_> {
        let buffer_version = buffer.version();
        let current_lines = buffer.lines_len();
        let snapshot_version = match self.snapshots.get(&buffer_id) {
            Some(snapshot) => snapshot.version,
            None => {
                self.render_cache.remove(&buffer_id);
                return RenderSnapshot {
                    snapshot: None,
                    line_map: None,
                };
            }
        };
        if snapshot_version == buffer_version {
            self.render_cache.remove(&buffer_id);
            return RenderSnapshot {
                snapshot: self.snapshots.get(&buffer_id),
                line_map: None,
            };
        }
        if snapshot_version > buffer_version {
            self.render_cache.remove(&buffer_id);
            return RenderSnapshot {
                snapshot: None,
                line_map: None,
            };
        }

        // Large buffers skip the line-map diff — the diff cost dominates the
        // perceived highlighting latency, so they keep rendering the latest
        // snapshot even if it lags the current version.
        let large = buffer.bytes_len() >= LARGE_MAP_BYTES || current_lines >= LARGE_MAP_LINES;
        if large {
            self.render_cache.remove(&buffer_id);
            return RenderSnapshot {
                snapshot: self.snapshots.get(&buffer_id),
                line_map: None,
            };
        }
        let cache_hit = self
            .render_cache
            .get(&buffer_id)
            .map(|cache| {
                cache.buffer_version == buffer_version && cache.snapshot_version == snapshot_version
            })
            .unwrap_or(false);
        if !cache_hit {
            let line_map = if self
                .full_reparse_pending
                .get(&buffer_id)
                .copied()
                .unwrap_or(false)
            {
                let current_hashes = self.line_hashes_for(buffer_id, buffer);
                let line_map = {
                    let snapshot = self.snapshots.get(&buffer_id).expect("snapshot");
                    build_line_map_by_hash(&snapshot.line_hashes, current_hashes.as_ref())
                };
                Some(line_map)
            } else {
                let edits = self
                    .edits_since_snapshot
                    .get(&buffer_id)
                    .map(|edits| {
                        edits
                            .iter()
                            .filter(|e| e.version > snapshot_version)
                            .map(|e| e.edit.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if edits.is_empty() {
                    let current_hashes = self.line_hashes_for(buffer_id, buffer);
                    if current_hashes.is_empty() {
                        None
                    } else {
                        let line_map = {
                            let snapshot = self.snapshots.get(&buffer_id).expect("snapshot");
                            build_line_map_by_hash(&snapshot.line_hashes, current_hashes.as_ref())
                        };
                        Some(line_map)
                    }
                } else {
                    let old_lines = self
                        .snapshots
                        .get(&buffer_id)
                        .map(|snap| snap.per_line.len())
                        .unwrap_or(0);
                    let line_map = build_line_map(old_lines, &edits);
                    let mut current_to_snapshot = vec![None; current_lines];
                    for (old_idx, new_idx) in line_map.into_iter().enumerate() {
                        if let Some(new_idx) = new_idx {
                            if new_idx < current_to_snapshot.len() {
                                current_to_snapshot[new_idx] = Some(old_idx);
                            }
                        }
                    }
                    Some(current_to_snapshot)
                }
            };
            if let Some(line_map) = line_map {
                self.render_cache.insert(
                    buffer_id,
                    RenderCache {
                        buffer_version,
                        snapshot_version,
                        line_map: Some(line_map),
                    },
                );
            } else {
                self.render_cache.remove(&buffer_id);
                return RenderSnapshot {
                    snapshot: self.snapshots.get(&buffer_id),
                    line_map: None,
                };
            }
        }
        let snapshot = self.snapshots.get(&buffer_id);
        let line_map = self
            .render_cache
            .get(&buffer_id)
            .and_then(|cache| cache.line_map.as_deref());
        RenderSnapshot { snapshot, line_map }
    }

    pub fn status_label_for(&self, buffer_id: usize, buffer_version: u64) -> String {
        let status = self.manager.status_for(buffer_id);
        if matches!(
            status,
            nit_syntax::SyntaxStatus::Ok(nit_syntax::EngineKind::TreeSitter)
        ) {
            let lagging = self
                .snapshots
                .get(&buffer_id)
                .map(|snap| snap.version < buffer_version)
                .unwrap_or(true);
            if lagging {
                return "TS(lag)".to_string();
            }
        }
        status.label().to_string()
    }

    pub fn engine_state_label(&self, buffer_id: usize) -> String {
        match self.manager.status_for(buffer_id) {
            nit_syntax::SyntaxStatus::Ok(_) => "ok".to_string(),
            nit_syntax::SyntaxStatus::Error(_) => "error".to_string(),
            nit_syntax::SyntaxStatus::Disabled => "off".to_string(),
        }
    }

    fn trim_edits_since_snapshot(&mut self, buffer_id: usize) {
        let Some(snapshot) = self.snapshots.get(&buffer_id) else {
            return;
        };
        if let Some(edits) = self.edits_since_snapshot.get_mut(&buffer_id) {
            edits.retain(|e| e.version > snapshot.version);
            if edits.is_empty() {
                self.edits_since_snapshot.remove(&buffer_id);
            }
        }
        self.full_reparse_pending.remove(&buffer_id);
    }

    fn line_hashes_for(&mut self, buffer_id: usize, buffer: &Buffer) -> Arc<[u64]> {
        let version = buffer.version();
        let stale = self
            .line_hash_cache
            .get(&buffer_id)
            .is_none_or(|cache| cache.version != version);
        if stale {
            let hashes = compute_buffer_line_hashes(buffer);
            self.line_hash_cache.insert(
                buffer_id,
                CachedLineHashes {
                    version,
                    hashes: Arc::from(hashes),
                },
            );
        }
        self.line_hash_cache
            .get(&buffer_id)
            .map(|cache| Arc::clone(&cache.hashes))
            .unwrap_or_else(|| Arc::from(Vec::new()))
    }
}

fn viewport_from_buffer(buffer: &Buffer) -> ViewportRange {
    ViewportRange {
        first_line: buffer.viewport.offset_line,
        last_line: buffer.viewport.offset_line + buffer.viewport.height.saturating_sub(1),
        total_lines: buffer.lines_len(),
    }
}

fn viewport_needs_rehighlight(
    current: &ViewportRange,
    highlighted_range: Option<(usize, usize)>,
) -> bool {
    match highlighted_range {
        Some((hl_start, hl_end)) => {
            // Rehighlight if viewport has scrolled within 20 lines of the highlighted range boundary
            current.first_line < hl_start.saturating_add(20)
                || current.last_line > hl_end.saturating_sub(20)
        }
        None => false, // Full file is highlighted (eager mode or progressive fill complete)
    }
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

fn build_line_map_by_hash(snapshot_hashes: &[u64], current_hashes: &[u64]) -> Vec<Option<usize>> {
    let mut map = vec![None; current_hashes.len()];
    let mut i = 0;
    let mut j = 0;
    const WINDOW: usize = 8;
    while i < snapshot_hashes.len() && j < current_hashes.len() {
        if snapshot_hashes[i] == current_hashes[j] {
            map[j] = Some(i);
            i += 1;
            j += 1;
            continue;
        }
        let mut next_i = None;
        for di in 1..=WINDOW {
            let idx = i + di;
            if idx >= snapshot_hashes.len() {
                break;
            }
            if snapshot_hashes[idx] == current_hashes[j] {
                next_i = Some(idx);
                break;
            }
        }
        let mut next_j = None;
        for dj in 1..=WINDOW {
            let idx = j + dj;
            if idx >= current_hashes.len() {
                break;
            }
            if current_hashes[idx] == snapshot_hashes[i] {
                next_j = Some(idx);
                break;
            }
        }
        match (next_i, next_j) {
            (Some(ni), Some(nj)) => {
                if ni - i <= nj - j {
                    i = ni;
                } else {
                    j = nj;
                }
            }
            (Some(ni), None) => {
                i = ni;
            }
            (None, Some(nj)) => {
                j = nj;
            }
            (None, None) => {
                j += 1;
            }
        }
    }
    map
}

fn compute_buffer_line_hashes(buffer: &Buffer) -> Vec<u64> {
    let lines = buffer.lines_len();
    let mut hashes = Vec::with_capacity(lines);
    for idx in 0..lines {
        hashes.push(buffer.line_hash(idx));
    }
    hashes
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

static HIGHLIGHT_SCHEDULE_LOG: OnceLock<Mutex<Instant>> = OnceLock::new();

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

fn adaptive_debounce_ms(base_ms: u64, bytes: usize) -> u64 {
    if bytes >= 1_500_000 {
        base_ms.max(500)
    } else if bytes >= 800_000 {
        base_ms.max(300)
    } else if bytes >= 300_000 {
        base_ms.max(150)
    } else {
        base_ms
    }
}

// Bug 2 fix: increase adaptive caps to avoid visible truncation in dense syntax.
fn adaptive_max_spans_per_line(base: usize, bytes: usize) -> usize {
    if bytes >= 1_500_000 {
        base.min(96)
    } else if bytes >= 800_000 {
        base.min(128)
    } else if bytes >= 300_000 {
        base.min(192)
    } else {
        base
    }
}

#[cfg(test)]
#[path = "tests/syntax.rs"]
mod tests;
