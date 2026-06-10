use nit_syntax::{
    HighlightRequest, HighlightSnapshot, LanguageId, LanguageRegistry, MappedLineSegment,
    SyntaxConfig, SyntaxEngine, SyntaxManager,
};
use ratatui::style::Style;
use ratatui::text::Span;
use std::{
    collections::{hash_map::DefaultHasher, VecDeque},
    hash::{Hash, Hasher},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthChar;

use crate::theme::Theme;

// Upper bound on the *first* (cold) code-block highlight: the worker has to
// build a tree-sitter config for every grammar before it can answer, which —
// with the full-taxonomy highlight queries — can take several seconds on a
// cold/slow machine (matches nit-syntax's own 15s cold-build test budget).
// Once the worker is warm this returns in well under a frame; the cap only
// ever bites the first block rendered in a session. A lazy per-language build
// would remove even that one-time wait (tracked as a follow-up).
const DOCUMENT_HIGHLIGHT_WAIT: Duration = Duration::from_secs(15);
const DOCUMENT_HIGHLIGHT_CACHE_LIMIT: usize = 96;

struct DocumentSyntaxHighlighter {
    manager: SyntaxManager,
    recent_buffer_ids: VecDeque<usize>,
}

impl DocumentSyntaxHighlighter {
    fn new() -> Self {
        Self {
            manager: SyntaxManager::new(SyntaxConfig::default()),
            recent_buffer_ids: VecDeque::new(),
        }
    }

    fn highlight(&mut self, language: LanguageId, text: &str) -> Option<HighlightSnapshot> {
        let (buffer_id, version) = syntax_cache_key(language, text);
        if let Some(snapshot) = self.manager.try_get_highlights(buffer_id, version) {
            self.touch_buffer_id(buffer_id);
            return Some(snapshot);
        }

        self.touch_buffer_id(buffer_id);
        self.manager.schedule_rehighlight(HighlightRequest {
            buffer_id,
            version,
            language,
            text: text.to_string(),
            edits: Vec::new(),
            full_reparse: true,
            max_spans_per_line: self.manager.config().max_spans_per_line,
            viewport: None,
        });
        wait_for_document_snapshot(
            &mut self.manager,
            buffer_id,
            version,
            DOCUMENT_HIGHLIGHT_WAIT,
        )
    }

    fn touch_buffer_id(&mut self, buffer_id: usize) {
        if let Some(pos) = self
            .recent_buffer_ids
            .iter()
            .position(|seen| *seen == buffer_id)
        {
            self.recent_buffer_ids.remove(pos);
        }
        self.recent_buffer_ids.push_back(buffer_id);
        if self.recent_buffer_ids.len() > DOCUMENT_HIGHLIGHT_CACHE_LIMIT {
            self.manager = SyntaxManager::new(SyntaxConfig::default());
            self.recent_buffer_ids.clear();
            self.recent_buffer_ids.push_back(buffer_id);
        }
    }
}

fn document_syntax_highlighter() -> &'static Mutex<DocumentSyntaxHighlighter> {
    static DOCUMENT_SYNTAX_HIGHLIGHTER: OnceLock<Mutex<DocumentSyntaxHighlighter>> =
        OnceLock::new();
    DOCUMENT_SYNTAX_HIGHLIGHTER.get_or_init(|| Mutex::new(DocumentSyntaxHighlighter::new()))
}

fn syntax_cache_key(language: LanguageId, text: &str) -> (usize, u64) {
    let mut hasher = DefaultHasher::new();
    language.hash(&mut hasher);
    text.hash(&mut hasher);
    let hash = hasher.finish();
    (hash as usize, hash)
}

fn wait_for_document_snapshot(
    manager: &mut SyntaxManager,
    buffer_id: usize,
    version: u64,
    timeout: Duration,
) -> Option<HighlightSnapshot> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(snapshot) = manager.try_get_highlights(buffer_id, version) {
            return Some(snapshot);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Resolve a fenced-code-block language tag via the central injection-alias
/// table — `rs` → `rust`, `cpp` → `cpp`, etc. Falls through to the raw
/// lowercased input when no alias matches so shell-session / plaintext tags
/// keep working through [`language_id_for_code_block`].
pub(super) fn canonical_code_lang(code_lang: &str) -> String {
    let trimmed = code_lang.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    nit_core::languages::detect_by_injection_alias(trimmed)
        .map(|info| info.label.to_string())
        .unwrap_or_else(|| trimmed.to_ascii_lowercase())
}

pub(super) fn is_json_code_lang(code_lang: &str) -> bool {
    // `canonical_code_lang` already folds jsonc/geojson into "json" via the
    // central injection-alias table — checking the canonical label is enough.
    canonical_code_lang(code_lang) == "json"
}

pub(super) fn highlight_code_block(code_lang: &str, lines: &[String]) -> Option<HighlightSnapshot> {
    let text = lines.join("\n");
    let language = language_id_for_code_block(code_lang, text.as_str())?;
    let mut highlighter = document_syntax_highlighter().lock().ok()?;
    highlighter.highlight(language, text.as_str())
}

fn language_id_for_code_block(code_lang: &str, text: &str) -> Option<LanguageId> {
    let normalized = canonical_code_lang(code_lang);
    if normalized.is_empty() {
        let trimmed = text.trim_start();
        if matches!(trimmed.chars().next(), Some('{') | Some('[')) {
            return Some(LanguageId::Json);
        }
        return None;
    }

    match normalized.as_str() {
        // `fish` lives in `LANGUAGES["bash"].extensions` but not in its
        // `injection_aliases`, so the central table won't resolve it from a
        // fenced-code tag. Keep a local branch until the alias is added.
        "fish" => Some(LanguageId::Bash),
        // `PlainText` is enum-only — explicit plain-text aliases never hit
        // any tree-sitter grammar so they bypass the central table.
        "text" | "txt" | "plaintext" | "plain" => Some(LanguageId::PlainText),
        _ => LanguageRegistry::from_injection_name(normalized.as_str()),
    }
}

pub(super) fn syntax_highlighted_wrapped_segments(
    text: &str,
    mapped: &[MappedLineSegment],
    base: Style,
    theme: &Theme,
    width: usize,
) -> Vec<Vec<Span<'static>>> {
    if text.is_empty() {
        return vec![vec![Span::styled(String::new(), base)]];
    }

    let chars: Vec<char> = text.chars().collect();
    let mut styles = vec![base; chars.len()];
    for seg in mapped {
        if seg.start >= seg.end || seg.start >= styles.len() {
            continue;
        }
        let style = base.patch(theme.highlight_style(seg.group));
        for idx in seg.start..seg.end.min(styles.len()) {
            styles[idx] = styles[idx].patch(style);
        }
    }

    let width = width.max(1);
    let mut out: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut buffers: Vec<String> = vec![String::new()];
    let mut current_styles = vec![styles[0]];
    let mut current_width = 0usize;
    let mut line_idx = 0usize;

    let flush_buffer = |out: &mut Vec<Vec<Span<'static>>>,
                        buffers: &mut Vec<String>,
                        current_styles: &[Style],
                        line_idx: usize| {
        if !buffers[line_idx].is_empty() {
            out[line_idx].push(Span::styled(
                std::mem::take(&mut buffers[line_idx]),
                current_styles[line_idx],
            ));
        }
    };

    let push_styled_char = |out: &mut Vec<Vec<Span<'static>>>,
                            buffers: &mut Vec<String>,
                            current_styles: &mut Vec<Style>,
                            line_idx: usize,
                            ch: char,
                            style: Style| {
        if style != current_styles[line_idx] && !buffers[line_idx].is_empty() {
            out[line_idx].push(Span::styled(
                std::mem::take(&mut buffers[line_idx]),
                current_styles[line_idx],
            ));
        }
        current_styles[line_idx] = style;
        buffers[line_idx].push(ch);
    };

    for (idx, ch) in chars.iter().enumerate() {
        let style = styles[idx];
        if *ch == '\t' {
            let tab_width = next_tab_width(current_width, width);
            for _ in 0..tab_width {
                if current_width + 1 > width && !buffers[line_idx].is_empty() {
                    flush_buffer(&mut out, &mut buffers, &current_styles, line_idx);
                    out.push(Vec::new());
                    buffers.push(String::new());
                    current_styles.push(style);
                    line_idx += 1;
                    current_width = 0;
                }
                push_styled_char(
                    &mut out,
                    &mut buffers,
                    &mut current_styles,
                    line_idx,
                    ' ',
                    style,
                );
                current_width += 1;
            }
            continue;
        }

        let ch_width = UnicodeWidthChar::width(*ch).unwrap_or(1).max(1);
        if current_width + ch_width > width && !buffers[line_idx].is_empty() {
            flush_buffer(&mut out, &mut buffers, &current_styles, line_idx);
            out.push(Vec::new());
            buffers.push(String::new());
            current_styles.push(style);
            line_idx += 1;
            current_width = 0;
        }
        push_styled_char(
            &mut out,
            &mut buffers,
            &mut current_styles,
            line_idx,
            *ch,
            style,
        );
        current_width += ch_width;
    }

    flush_buffer(&mut out, &mut buffers, &current_styles, line_idx);
    if out.is_empty() {
        vec![vec![Span::styled(String::new(), base)]]
    } else {
        out
    }
}

pub(super) fn next_tab_width(col: usize, width: usize) -> usize {
    let width = width.max(1);
    let to_stop = 4usize.saturating_sub(col % 4);
    to_stop.max(1).min(width)
}
