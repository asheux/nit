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
    ) {
        let _ = self.cmd_tx.send(PreviewCommand::Request {
            generation,
            mode,
            path,
            line_hint,
            query,
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

fn preview_loop(
    cmd_rx: Receiver<PreviewCommand>,
    event_tx: Sender<PreviewEvent>,
    theme: Theme,
    initial_config: HighlightConfig,
) {
    let mut config = initial_config;
    let mut manager = SyntaxManager::new(to_syntax_config(config.clone()));
    let mut version = 0u64;

    let mut cache: HashMap<PreviewKey, PreviewModel> = HashMap::new();
    let mut order: VecDeque<PreviewKey> = VecDeque::new();

    loop {
        let cmd = match cmd_rx.recv() {
            Ok(cmd) => cmd,
            Err(_) => break,
        };

        let mut request = match cmd {
            PreviewCommand::Request {
                generation,
                mode,
                path,
                line_hint,
                query,
            } => Some((generation, mode, path, line_hint, query)),
            PreviewCommand::UpdateConfig { config: cfg } => {
                config = cfg;
                manager.update_config(to_syntax_config(config.clone()));
                cache.clear();
                order.clear();
                None
            }
            PreviewCommand::Shutdown => break,
        };

        while let Ok(next) = cmd_rx.try_recv() {
            match next {
                PreviewCommand::Request {
                    generation,
                    mode,
                    path,
                    line_hint,
                    query,
                } => {
                    request = Some((generation, mode, path, line_hint, query));
                }
                PreviewCommand::UpdateConfig { config: cfg } => {
                    config = cfg;
                    manager.update_config(to_syntax_config(config.clone()));
                    cache.clear();
                    order.clear();
                }
                PreviewCommand::Shutdown => return,
            }
        }

        let Some((generation, mode, path, line_hint, query)) = request else {
            continue;
        };
        let key = PreviewKey {
            path: path.clone(),
            mode,
            line_hint: line_hint.unwrap_or(0),
            query: if matches!(mode, SearchMode::Content) {
                query.clone()
            } else {
                String::new()
            },
        };
        if let Some(model) = cache.get(&key).cloned() {
            touch_lru(&mut order, &key);
            let _ = event_tx.send(PreviewEvent::Ready { generation, model });
            continue;
        }

        let model = match build_preview(
            &path,
            mode,
            line_hint,
            if matches!(mode, SearchMode::Content) {
                query.as_str()
            } else {
                ""
            },
            &theme,
            &mut manager,
            &mut version,
            config.enabled,
        ) {
            Ok(model) => model,
            Err(err) => {
                let _ = event_tx.send(PreviewEvent::Error {
                    generation,
                    message: err,
                });
                continue;
            }
        };

        cache.insert(key.clone(), model.clone());
        order.push_back(key);
        while order.len() > PREVIEW_CACHE_CAP {
            if let Some(old) = order.pop_front() {
                cache.remove(&old);
            }
        }

        let _ = event_tx.send(PreviewEvent::Ready { generation, model });
    }
}

fn touch_lru(order: &mut VecDeque<PreviewKey>, key: &PreviewKey) {
    if let Some(pos) = order.iter().position(|k| k == key) {
        if let Some(item) = order.remove(pos) {
            order.push_back(item);
        }
    }
}

fn build_preview(
    path: &Path,
    mode: SearchMode,
    line_hint: Option<usize>,
    query: &str,
    theme: &Theme,
    manager: &mut SyntaxManager,
    version: &mut u64,
    highlight_enabled: bool,
) -> Result<PreviewModel, String> {
    let (start_line, anchor_line) = match mode {
        SearchMode::Files => (1usize, 0usize),
        SearchMode::Content => {
            let hint = line_hint.unwrap_or(1).max(1);
            let start = hint.saturating_sub(PREVIEW_CONTEXT_BEFORE).max(1);
            (start, hint.saturating_sub(start))
        }
    };

    let (lines, truncated) = read_window(path, start_line, PREVIEW_LINES)?;
    let text = lines.join("\n");
    let snapshot = if highlight_enabled {
        *version = version.wrapping_add(1);
        let language = manager.detect_language(Some(path), lines.first().map(|s| s.as_str()), None);
        let request = HighlightRequest {
            buffer_id: 0,
            version: *version,
            language,
            text: text.clone(),
            edits: Vec::new(),
            full_reparse: true,
            max_spans_per_line: manager.config().max_spans_per_line,
        };
        manager.schedule_rehighlight(request);
        wait_for_snapshot(manager, 0, *version, HIGHLIGHT_WAIT)
    } else {
        None
    };

    let mut styled_lines: Vec<Line<'static>> = Vec::with_capacity(lines.len().max(1));
    for (idx, line) in lines.iter().enumerate() {
        let mapped = snapshot
            .as_ref()
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

    Ok(PreviewModel {
        path: path.to_path_buf(),
        start_line,
        anchor_line: anchor_line.min(styled_lines.len().saturating_sub(1)),
        truncated,
        lines: styled_lines,
    })
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
        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
        }
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
    if let Some(segments) = mapped {
        for seg in segments {
            let seg_style = theme.highlight_style(seg.group).bg(theme.background);
            let end = seg.end.min(styles.len());
            for s in styles.iter_mut().take(end).skip(seg.start) {
                *s = seg_style;
            }
        }
    }
    if !overlay.is_empty() {
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
mod tests {
    use super::*;

    #[test]
    fn find_matches_returns_multiple_ranges() {
        let line = "abc main xyz main";
        let ranges = find_matches(line, "main");
        assert_eq!(ranges, vec![(4, 8), (13, 17)]);
    }

    #[test]
    fn find_matches_handles_unicode_char_boundaries() {
        let line = "αβγδε";
        let ranges = find_matches(line, "βγ");
        assert_eq!(ranges, vec![(1, 3)]);
    }
}
