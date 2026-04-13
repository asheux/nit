//! Highlight output types and span-to-line distribution.
//!
//! Data flow: tree-sitter produces [`HighlightSpan`]s covering byte ranges
//! → [`sort_spans`] orders them → [`distribute_spans_to_lines`] splits them
//! into per-line [`LineSegment`]s → [`map_line_segments_to_chars`] converts
//! byte offsets to character indices for the renderer.

use std::cmp::Ordering;

use crate::registry::LanguageId;

// ── Highlight groups ────────────────────────────────────────────────────────

/// Semantic highlight category mapped to theme colours by the renderer.
///
/// Variant order is stable — new entries are appended at the end.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HighlightGroup {
    Normal,
    Comment,
    DocComment,
    String,
    Char,
    Number,
    Boolean,
    Keyword,
    KeywordControl,
    KeywordOperator,
    Type,
    TypeBuiltin,
    Function,
    Method,
    Macro,
    Attribute,
    Namespace,
    Variable,
    Parameter,
    Property,
    Constant,
    Operator,
    Punctuation,
    Tag,
    Heading,
    Emphasis,
    Link,
    Error,
    Warning,
    DiffAdd,
    DiffRemove,
}

// ── Engine identification ───────────────────────────────────────────────────

/// Identifies which highlighting backend produced the result.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EngineKind {
    TreeSitter,
    Plain,
}

/// Current operational state of highlighting for a buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyntaxStatus {
    Ok(EngineKind),
    Disabled,
    Error(String),
}

impl SyntaxStatus {
    /// Human-readable status-bar label.
    pub fn label(&self) -> String {
        match self {
            Self::Ok(EngineKind::TreeSitter) => "TS(ok)".to_string(),
            Self::Ok(EngineKind::Plain) => "Plain(ok)".to_string(),
            Self::Disabled => "Off".to_string(),
            Self::Error(_) => "TS(error)".to_string(),
        }
    }
}

// ── Span and segment types ──────────────────────────────────────────────────

/// Byte-range highlight span before per-line splitting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub group: HighlightGroup,
    pub priority: u8,
    pub modifiers: u8,
}

/// Byte-range segment relative to a single source line.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LineSegment {
    pub start: usize,
    pub end: usize,
    pub group: HighlightGroup,
}

/// Character-index segment produced by [`map_line_segments_to_chars`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MappedLineSegment {
    pub start: usize,
    pub end: usize,
    pub group: HighlightGroup,
}

/// Error when a segment boundary falls inside a multi-byte character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentMapError {
    pub start: usize,
    pub end: usize,
    pub line_len: usize,
}

// ── Snapshot ────────────────────────────────────────────────────────────────

/// Immutable, versioned output of a highlight pass for one buffer.
#[derive(Clone, Debug)]
pub struct HighlightSnapshot {
    pub buffer_id: usize,
    pub version: u64,
    pub language: LanguageId,
    pub engine: EngineKind,
    pub status: SyntaxStatus,
    pub duration_ms: u128,
    pub line_start_bytes: Vec<usize>,
    pub line_hashes: Vec<u64>,
    pub per_line: Vec<Vec<LineSegment>>,
    /// `Some((start, end))` for viewport-scoped; `None` for full-file.
    pub highlighted_range: Option<(usize, usize)>,
}

impl HighlightSnapshot {
    /// Construct a snapshot with zero highlight spans (plain-text fallback).
    pub fn plain(
        buffer_id: usize,
        version: u64,
        language: LanguageId,
        engine: EngineKind,
        status: SyntaxStatus,
        text: &str,
    ) -> Self {
        let offsets = compute_line_starts(text);
        let hashes = compute_line_hashes(text, &offsets);

        Self {
            buffer_id,
            version,
            language,
            engine,
            status,
            duration_ms: 0,
            per_line: vec![Vec::new(); hashes.len()],
            line_start_bytes: offsets,
            line_hashes: hashes,
            highlighted_range: None,
        }
    }

    /// Build a snapshot by sorting raw spans and distributing them across
    /// line boundaries into per-line segment lists.
    #[allow(clippy::too_many_arguments)]
    pub fn from_spans(
        buffer_id: usize,
        version: u64,
        language: LanguageId,
        engine: EngineKind,
        status: SyntaxStatus,
        text: &str,
        mut spans: Vec<HighlightSpan>,
        max_per_line: usize,
    ) -> Self {
        sort_spans(&mut spans);

        let offsets = compute_line_starts(text);
        let mut lines = vec![Vec::new(); offsets.len().saturating_sub(1)];

        distribute_spans_to_lines(&spans, &offsets, &mut lines, max_per_line, |_| true);

        let hashes = compute_line_hashes(text, &offsets);

        Self {
            buffer_id,
            version,
            language,
            engine,
            status,
            duration_ms: 0,
            line_start_bytes: offsets,
            line_hashes: hashes,
            per_line: lines,
            highlighted_range: None,
        }
    }
}

// ── FNV-1a line hashing ─────────────────────────────────────────────────────

/// Compute an FNV-1a hash of a source line, stripping trailing `\n` and
/// ignoring `\r` so that hashes are line-ending-agnostic.
pub fn hash_line_bytes(raw: &[u8]) -> u64 {
    const BASIS: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;

    let end = if raw.last() == Some(&b'\n') {
        raw.len() - 1
    } else {
        raw.len()
    };

    raw[..end]
        .iter()
        .filter(|&&b| b != b'\r')
        .fold(BASIS, |hash, &b| (hash ^ b as u64).wrapping_mul(PRIME))
}

/// Recompute [`hash_line_bytes`] for a set of lines, writing results
/// directly into `hashes`. Safely skips out-of-range indices.
pub(crate) fn rehash_lines(
    text: &[u8],
    line_starts: &[usize],
    hashes: &mut [u64],
    lines: impl Iterator<Item = usize>,
) {
    for i in lines {
        if i + 1 < line_starts.len() && i < hashes.len() {
            hashes[i] = hash_line_bytes(&text[line_starts[i]..line_starts[i + 1]]);
        }
    }
}

/// Hash every line in the buffer using [`hash_line_bytes`].
fn compute_line_hashes(text: &str, offsets: &[usize]) -> Vec<u64> {
    let bytes = text.as_bytes();
    (0..offsets.len().saturating_sub(1))
        .map(|i| hash_line_bytes(&bytes[offsets[i]..offsets[i + 1]]))
        .collect()
}

// ── Byte→char segment mapping ───────────────────────────────────────────────

/// Map a single byte-offset segment to character indices via binary search.
fn resolve_segment_chars(
    seg: &LineSegment,
    byte_len: usize,
    boundaries: &[usize],
) -> Result<Option<MappedLineSegment>, SegmentMapError> {
    let start = seg.start;
    let end = seg.end.min(byte_len);

    if start >= byte_len || start >= end {
        return Ok(None);
    }

    let err = || SegmentMapError {
        start,
        end,
        line_len: byte_len,
    };

    let char_start = boundaries.binary_search(&start).map_err(|_| err())?;
    let char_end = boundaries.binary_search(&end).map_err(|_| err())?;

    Ok((char_start < char_end).then_some(MappedLineSegment {
        start: char_start,
        end: char_end,
        group: seg.group,
    }))
}

/// Convert byte-offset [`LineSegment`]s to character-index
/// [`MappedLineSegment`]s. Fails if any boundary falls inside a multi-byte
/// character.
pub fn map_line_segments_to_chars(
    line: &str,
    segments: &[LineSegment],
) -> Result<Vec<MappedLineSegment>, SegmentMapError> {
    if segments.is_empty() || line.is_empty() {
        return Ok(Vec::new());
    }

    let byte_len = line.len();
    let mut boundaries: Vec<usize> = line.char_indices().map(|(pos, _)| pos).collect();
    boundaries.push(byte_len);

    let mut result = Vec::with_capacity(segments.len());
    for seg in segments {
        if let Some(mapped) = resolve_segment_chars(seg, byte_len, &boundaries)? {
            result.push(mapped);
        }
    }
    Ok(result)
}

// ── Span sorting ────────────────────────────────────────────────────────────

/// Sort highlight spans in source order: `start_byte` ascending, `priority`
/// descending for ties. Skips the sort when already ordered.
pub(crate) fn sort_spans(spans: &mut [HighlightSpan]) {
    spans.sort_by(|a, b| match a.start_byte.cmp(&b.start_byte) {
        Ordering::Equal => b.priority.cmp(&a.priority),
        ord => ord,
    });
}

// ── Span→line distribution ──────────────────────────────────────────────────

/// Distribute pre-sorted [`HighlightSpan`]s into per-line segment lists.
///
/// Only lines where `predicate(line_index)` returns `true` receive segments.
pub(crate) fn distribute_spans_to_lines(
    spans: &[HighlightSpan],
    offsets: &[usize],
    segments: &mut [Vec<LineSegment>],
    max_per_line: usize,
    predicate: impl Fn(usize) -> bool,
) {
    for span in spans {
        if span.end_byte <= span.start_byte {
            continue;
        }
        assign_span_to_lines(span, offsets, segments, max_per_line, &predicate);
    }
}

/// Clamp a single highlight span to each intersecting line and append the
/// resulting line-relative segment.
fn assign_span_to_lines(
    span: &HighlightSpan,
    offsets: &[usize],
    segments: &mut [Vec<LineSegment>],
    max_per_line: usize,
    predicate: &impl Fn(usize) -> bool,
) {
    let first = find_line(offsets, span.start_byte);
    let last = find_line(offsets, span.end_byte);

    for line in first..=last {
        if line + 1 >= offsets.len() || line >= segments.len() {
            break;
        }
        if !predicate(line) {
            continue;
        }
        if max_per_line > 0 && segments[line].len() >= max_per_line {
            continue;
        }

        let line_start = offsets[line];
        let line_end = offsets[line + 1];
        let seg_start = span.start_byte.max(line_start) - line_start;
        let seg_end = span.end_byte.min(line_end) - line_start;

        if seg_start < seg_end {
            segments[line].push(LineSegment {
                start: seg_start,
                end: seg_end,
                group: span.group,
            });
        }
    }
}

// ── Line offset index ───────────────────────────────────────────────────────

/// Build a sorted table of byte offsets where each source line begins.
/// The final entry equals `text.len()`.
pub(crate) fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];

    offsets.extend(
        text.bytes()
            .enumerate()
            .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
    );

    let last = *offsets.last().unwrap_or(&0);
    if last != text.len() {
        offsets.push(text.len());
    }

    offsets
}

/// Binary-search the line offset table to find which line contains
/// `target_byte`. Returns the zero-based line index.
pub(crate) fn find_line(offsets: &[usize], target_byte: usize) -> usize {
    offsets
        .partition_point(|&boundary| boundary <= target_byte)
        .saturating_sub(1)
}
