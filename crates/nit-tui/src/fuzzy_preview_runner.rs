use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};
use nit_core::{HighlightConfig, HighlightEngine, SearchMode};
use nit_syntax::{
    map_line_segments_to_chars, HighlightRequest, HighlightSnapshot, SyntaxEngine, SyntaxManager,
};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::Theme;

const PREVIEW_LINES: usize = 240;
const PREVIEW_CONTEXT_BEFORE: usize = 80;
const PREVIEW_MAX_BYTES: usize = 256 * 1024;
const PREVIEW_MAX_LINE_BYTES: usize = 4 * 1024;
const PREVIEW_CACHE_CAP: usize = 32;
const HIGHLIGHT_WAIT: Duration = Duration::from_millis(180);
const TAB_WIDTH: usize = 4;

#[derive(Clone, Debug)]
pub struct PreviewModel {
    pub path: PathBuf,
    pub start_line: usize,
    pub anchor_line: usize,
    pub truncated: bool,
    pub lines: Vec<Line<'static>>,
}

pub enum PreviewCommand {
    Request {
        generation: u64,
        mode: SearchMode,
        path: PathBuf,
        line_hint: Option<usize>,
        query: String,
        /// Live dirty-buffer content for `path` when an open editor
        /// buffer matches the path and has unsaved edits. When
        /// `Some`, the preview reads from this string instead of
        /// re-reading the on-disk file — closes the stale-preview
        /// gap operators saw via Ctrl+F / Ctrl+P after edits.
        override_content: Option<String>,
    },
    UpdateConfig {
        config: HighlightConfig,
    },
    Shutdown,
}

pub enum PreviewEvent {
    Ready {
        generation: u64,
        model: PreviewModel,
    },
    Error {
        generation: u64,
        message: String,
    },
}

pub struct PreviewRunner {
    cmd_tx: Sender<PreviewCommand>,
    pub events: Receiver<PreviewEvent>,
    handle: Option<JoinHandle<()>>,
}

impl PreviewRunner {
    pub fn spawn(theme: Theme, config: HighlightConfig) -> Self {
        let (cmd_tx, cmd_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let handle = thread::Builder::new()
            .name("nit-search-preview".into())
            .spawn(move || preview_loop(cmd_rx, event_tx, theme, config))
            .expect("spawn preview runner");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn request(
        &self,
        generation: u64,
        mode: SearchMode,
        path: PathBuf,
        line_hint: Option<usize>,
        query: String,
        override_content: Option<String>,
    ) {
        let _ = self.cmd_tx.send(PreviewCommand::Request {
            generation,
            mode,
            path,
            line_hint,
            query,
            override_content,
        });
    }

    pub fn update_config(&self, config: HighlightConfig) {
        let _ = self.cmd_tx.send(PreviewCommand::UpdateConfig { config });
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(PreviewCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct PreviewKey {
    path: PathBuf,
    mode: SearchMode,
    line_hint: usize,
    query: String,
}

struct PendingRequest {
    generation: u64,
    mode: SearchMode,
    path: PathBuf,
    line_hint: Option<usize>,
    query: String,
    override_content: Option<String>,
}

struct PreviewCache {
    entries: HashMap<PreviewKey, PreviewModel>,
    order: VecDeque<PreviewKey>,
}

impl PreviewCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    fn get_and_touch(&mut self, key: &PreviewKey) -> Option<PreviewModel> {
        let model = self.entries.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            if let Some(item) = self.order.remove(pos) {
                self.order.push_back(item);
            }
        }
        Some(model)
    }

    fn insert(&mut self, key: PreviewKey, model: PreviewModel) {
        self.entries.insert(key.clone(), model);
        self.order.push_back(key);
        while self.order.len() > PREVIEW_CACHE_CAP {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
    }
}

enum ApplyOutcome {
    Request(PendingRequest),
    Configured,
    Shutdown,
}

fn preview_loop(
    cmd_rx: Receiver<PreviewCommand>,
    event_tx: Sender<PreviewEvent>,
    theme: Theme,
    initial_config: HighlightConfig,
) {
    let mut config = initial_config;
    let mut manager = SyntaxManager::new(to_syntax_config(config.clone()));
    let mut version = 0u64;
    let mut cache = PreviewCache::new();

    while let Ok(cmd) = cmd_rx.recv() {
        match coalesce_pending(cmd, &cmd_rx, &mut config, &mut manager, &mut cache) {
            ApplyOutcome::Request(request) => serve_request(
                request,
                &theme,
                &mut manager,
                &mut version,
                &mut cache,
                config.enabled,
                &event_tx,
            ),
            ApplyOutcome::Configured => {}
            ApplyOutcome::Shutdown => return,
        }
    }
}

fn serve_request(
    request: PendingRequest,
    theme: &Theme,
    manager: &mut SyntaxManager,
    version: &mut u64,
    cache: &mut PreviewCache,
    highlight_enabled: bool,
    event_tx: &Sender<PreviewEvent>,
) {
    let key = preview_key_for(&request);
    if let Some(model) = cache.get_and_touch(&key) {
        let _ = event_tx.send(PreviewEvent::Ready {
            generation: request.generation,
            model,
        });
        return;
    }

    let query_for_build = if matches!(request.mode, SearchMode::Content) {
        request.query.as_str()
    } else {
        ""
    };
    match build_preview(
        &request.path,
        request.mode,
        request.line_hint,
        query_for_build,
        theme,
        manager,
        version,
        highlight_enabled,
        request.override_content.as_deref(),
    ) {
        Ok(model) => {
            cache.insert(key, model.clone());
            let _ = event_tx.send(PreviewEvent::Ready {
                generation: request.generation,
                model,
            });
        }
        Err(err) => {
            let _ = event_tx.send(PreviewEvent::Error {
                generation: request.generation,
                message: err,
            });
        }
    }
}

/// Drain queued commands, keeping the most-recent request and short-circuiting
/// on shutdown. Config updates are applied in place.
fn coalesce_pending(
    initial: PreviewCommand,
    cmd_rx: &Receiver<PreviewCommand>,
    config: &mut HighlightConfig,
    manager: &mut SyntaxManager,
    cache: &mut PreviewCache,
) -> ApplyOutcome {
    let mut latest = apply_command(initial, config, manager, cache);
    if matches!(latest, ApplyOutcome::Shutdown) {
        return latest;
    }
    while let Ok(next) = cmd_rx.try_recv() {
        match apply_command(next, config, manager, cache) {
            ApplyOutcome::Shutdown => return ApplyOutcome::Shutdown,
            ApplyOutcome::Request(req) => latest = ApplyOutcome::Request(req),
            ApplyOutcome::Configured => {}
        }
    }
    latest
}

fn apply_command(
    cmd: PreviewCommand,
    config: &mut HighlightConfig,
    manager: &mut SyntaxManager,
    cache: &mut PreviewCache,
) -> ApplyOutcome {
    match cmd {
        PreviewCommand::Request {
            generation,
            mode,
            path,
            line_hint,
            query,
            override_content,
        } => ApplyOutcome::Request(PendingRequest {
            generation,
            mode,
            path,
            line_hint,
            query,
            override_content,
        }),
        PreviewCommand::UpdateConfig { config: cfg } => {
            *config = cfg;
            manager.update_config(to_syntax_config(config.clone()));
            cache.clear();
            ApplyOutcome::Configured
        }
        PreviewCommand::Shutdown => ApplyOutcome::Shutdown,
    }
}

fn preview_key_for(req: &PendingRequest) -> PreviewKey {
    PreviewKey {
        path: req.path.clone(),
        mode: req.mode,
        line_hint: req.line_hint.unwrap_or(0),
        query: if matches!(req.mode, SearchMode::Content) {
            req.query.clone()
        } else {
            String::new()
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn build_preview(
    path: &Path,
    mode: SearchMode,
    line_hint: Option<usize>,
    query: &str,
    theme: &Theme,
    manager: &mut SyntaxManager,
    version: &mut u64,
    highlight_enabled: bool,
    override_content: Option<&str>,
) -> Result<PreviewModel, String> {
    let (start_line, anchor_line) = preview_window_anchors(mode, line_hint);
    // Prefer the live dirty-buffer content when the caller supplied it;
    // skip the disk read so unsaved edits show up in the preview.
    let (lines, truncated) = match override_content {
        Some(content) => window_from_string(content, start_line, PREVIEW_LINES),
        None => read_window(path, start_line, PREVIEW_LINES)?,
    };
    let snapshot = highlight_enabled
        .then(|| highlight_snapshot(path, &lines, manager, version))
        .flatten();

    let styled_lines = stylize_lines(&lines, snapshot.as_ref(), mode, query, theme);

    Ok(PreviewModel {
        path: path.to_path_buf(),
        start_line,
        anchor_line: anchor_line.min(styled_lines.len().saturating_sub(1)),
        truncated,
        lines: styled_lines,
    })
}

fn preview_window_anchors(mode: SearchMode, line_hint: Option<usize>) -> (usize, usize) {
    match mode {
        SearchMode::Files => (1, 0),
        SearchMode::Content => {
            let hint = line_hint.unwrap_or(1).max(1);
            let start = hint.saturating_sub(PREVIEW_CONTEXT_BEFORE).max(1);
            (start, hint.saturating_sub(start))
        }
    }
}

fn highlight_snapshot(
    path: &Path,
    lines: &[String],
    manager: &mut SyntaxManager,
    version: &mut u64,
) -> Option<HighlightSnapshot> {
    *version = version.wrapping_add(1);
    let language = manager.detect_language(Some(path), lines.first().map(String::as_str), None);
    let request = HighlightRequest {
        buffer_id: 0,
        version: *version,
        language,
        text: lines.join("\n"),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: manager.config().max_spans_per_line,
        viewport: None,
    };
    manager.schedule_rehighlight(request);
    wait_for_snapshot(manager, 0, *version, HIGHLIGHT_WAIT)
}

fn stylize_lines(
    lines: &[String],
    snapshot: Option<&HighlightSnapshot>,
    mode: SearchMode,
    query: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut styled_lines: Vec<Line<'static>> = Vec::with_capacity(lines.len().max(1));
    for (idx, line) in lines.iter().enumerate() {
        let mapped = snapshot
            .and_then(|snap| snap.per_line.get(idx))
            .and_then(|segs| map_line_segments_to_chars(line, segs).ok());

        let overlay_ranges = if mode == SearchMode::Content && !query.is_empty() {
            find_matches(line, query)
        } else {
            Vec::new()
        };

        styled_lines.push(styled_line(line, mapped.as_deref(), &overlay_ranges, theme));
    }
    if styled_lines.is_empty() {
        styled_lines.push(Line::from(Span::styled(
            String::new(),
            Style::default().fg(theme.foreground).bg(theme.background),
        )));
    }
    styled_lines
}

fn wait_for_snapshot(
    manager: &mut SyntaxManager,
    buffer_id: usize,
    version: u64,
    timeout: Duration,
) -> Option<HighlightSnapshot> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(snap) = manager.try_get_highlights(buffer_id, version) {
            return Some(snap);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

// Same windowing semantics as `read_window` but over an in-memory
// string — used when the caller (the search UI) has a dirty editor
// buffer for the path and wants the preview to reflect unsaved
// edits. Truncation rules match `read_window` so a content vs
// disk source can be swapped without the operator-visible window
// changing shape.
fn window_from_string(content: &str, start_line: usize, max_lines: usize) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut total_bytes = 0usize;
    for (idx, mut line) in content
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.to_string()))
    {
        if out.len() >= max_lines {
            break;
        }
        if idx < start_line {
            continue;
        }
        if line.len() > PREVIEW_MAX_LINE_BYTES {
            line.truncate(PREVIEW_MAX_LINE_BYTES);
            truncated = true;
        }
        total_bytes = total_bytes.saturating_add(line.len());
        if total_bytes > PREVIEW_MAX_BYTES {
            truncated = true;
            break;
        }
        out.push(line);
    }
    (out, truncated)
}

fn read_window(
    path: &Path,
    start_line: usize,
    max_lines: usize,
) -> Result<(Vec<String>, bool), String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("Failed to open {}: {e}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut buf = String::new();
    let mut current = 0usize;
    let mut out: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut total_bytes = 0usize;
    while out.len() < max_lines {
        buf.clear();
        let n = std::io::BufRead::read_line(&mut reader, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        current += 1;
        if current < start_line {
            continue;
        }
        strip_trailing_newline(&mut buf);
        if buf.len() > PREVIEW_MAX_LINE_BYTES {
            buf.truncate(PREVIEW_MAX_LINE_BYTES);
            truncated = true;
        }
        total_bytes = total_bytes.saturating_add(buf.len());
        if total_bytes > PREVIEW_MAX_BYTES {
            truncated = true;
            break;
        }
        out.push(buf.clone());
    }
    Ok((out, truncated))
}

fn strip_trailing_newline(buf: &mut String) {
    if buf.ends_with('\n') {
        buf.pop();
        if buf.ends_with('\r') {
            buf.pop();
        }
    }
}

fn find_matches(line: &str, query: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let q_len = query.chars().count().max(1);
    for (byte_idx, _) in line.match_indices(query) {
        let start = line[..byte_idx].chars().count();
        ranges.push((start, start + q_len));
    }
    ranges
}

fn styled_line(
    line: &str,
    mapped: Option<&[nit_syntax::MappedLineSegment]>,
    overlay: &[(usize, usize)],
    theme: &Theme,
) -> Line<'static> {
    let default_style = Style::default().fg(theme.foreground).bg(theme.background);
    if line.is_empty() {
        return Line::from(Span::styled(String::new(), default_style));
    }

    let chars: Vec<char> = line.chars().collect();
    let mut styles = vec![default_style; chars.len()];
    apply_syntax_styles(&mut styles, mapped, theme);
    apply_overlay_styles(&mut styles, overlay, theme);
    // Expand TABs to spaces. `UnicodeWidthStr::width("\t")` returns 1, so ratatui's
    // Paragraph writes `\t` into the buffer cell verbatim; crossterm then `Print`s
    // it and the terminal jumps to the next tab stop, leaving the cells the diff
    // believed it had overwritten still showing the previous frame's chars.
    let (chars, styles) = expand_tabs(chars, styles);
    flatten_runs(chars, styles)
}

fn expand_tabs(chars: Vec<char>, styles: Vec<Style>) -> (Vec<char>, Vec<Style>) {
    if !chars.contains(&'\t') {
        return (chars, styles);
    }
    let mut out_chars = Vec::with_capacity(chars.len());
    let mut out_styles = Vec::with_capacity(styles.len());
    let mut col = 0usize;
    for (ch, style) in chars.into_iter().zip(styles.into_iter()) {
        if ch == '\t' {
            let advance = TAB_WIDTH - (col % TAB_WIDTH);
            for _ in 0..advance {
                out_chars.push(' ');
                out_styles.push(style);
            }
            col += advance;
        } else {
            out_chars.push(ch);
            out_styles.push(style);
            col += 1;
        }
    }
    (out_chars, out_styles)
}

fn apply_syntax_styles(
    styles: &mut [Style],
    mapped: Option<&[nit_syntax::MappedLineSegment]>,
    theme: &Theme,
) {
    let Some(segments) = mapped else { return };
    for seg in segments {
        let seg_style = theme.highlight_style(seg.group).bg(theme.background);
        let end = seg.end.min(styles.len());
        for s in styles.iter_mut().take(end).skip(seg.start) {
            *s = seg_style;
        }
    }
}

fn apply_overlay_styles(styles: &mut [Style], overlay: &[(usize, usize)], theme: &Theme) {
    if overlay.is_empty() {
        return;
    }
    for (start, end) in overlay {
        let start = (*start).min(styles.len());
        let end = (*end).min(styles.len());
        for s in styles.iter_mut().take(end).skip(start) {
            *s = s
                .bg(theme.selection_bg)
                .add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
        }
    }
}

fn flatten_runs(chars: Vec<char>, styles: Vec<Style>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = styles[0];
    let mut buf = String::new();
    for (ch, style) in chars.into_iter().zip(styles.into_iter()) {
        if style != current_style && !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut buf), current_style));
            current_style = style;
        }
        buf.push(ch);
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, current_style));
    }
    Line::from(spans)
}

fn to_syntax_config(config: HighlightConfig) -> nit_syntax::SyntaxConfig {
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

#[cfg(test)]
#[path = "tests/fuzzy_preview_runner.rs"]
mod tests;
