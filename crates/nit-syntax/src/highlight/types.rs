//! Output data types for the syntax pipeline: highlight groups, per-buffer
//! snapshots, and the byte/char segment shapes that flow between them.

use crate::language::LanguageId;

use super::lines::{compute_line_starts, recompute_line_hashes};
use super::spans::{distribute_spans_to_lines, sort_spans};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum HighlightGroup {
    #[default]
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EngineKind {
    TreeSitter,
    Plain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyntaxStatus {
    Ok(EngineKind),
    Disabled,
    Error(String),
}

impl SyntaxStatus {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ok(EngineKind::TreeSitter) => "TS(ok)",
            Self::Ok(EngineKind::Plain) => "Plain(ok)",
            Self::Disabled => "Off",
            Self::Error(_) => "TS(error)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub group: HighlightGroup,
    pub priority: u8,
    pub modifiers: u8,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LineSegment {
    pub start: usize,
    pub end: usize,
    pub group: HighlightGroup,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MappedLineSegment {
    pub start: usize,
    pub end: usize,
    pub group: HighlightGroup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentMapError {
    pub start: usize,
    pub end: usize,
    pub line_len: usize,
}

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
    /// `Some((start, end))` for viewport-scoped partial coverage; `None` once
    /// progressive fill completes and the whole buffer is highlighted.
    pub highlighted_range: Option<(usize, usize)>,
}

impl HighlightSnapshot {
    pub fn plain(
        buffer_id: usize,
        version: u64,
        language: LanguageId,
        engine: EngineKind,
        status: SyntaxStatus,
        text: &str,
    ) -> Self {
        let offsets = compute_line_starts(text);
        let line_count = offsets.len().saturating_sub(1);
        let mut hashes = vec![0u64; line_count];
        recompute_line_hashes(text.as_bytes(), &offsets, &mut hashes, 0..line_count);

        Self {
            buffer_id,
            version,
            language,
            engine,
            status,
            duration_ms: 0,
            per_line: vec![Vec::new(); line_count],
            line_start_bytes: offsets,
            line_hashes: hashes,
            highlighted_range: None,
        }
    }

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
        let line_count = offsets.len().saturating_sub(1);
        let mut per_line = vec![Vec::new(); line_count];
        distribute_spans_to_lines(&spans, &offsets, &mut per_line, max_per_line, |_| true);

        let mut hashes = vec![0u64; line_count];
        recompute_line_hashes(text.as_bytes(), &offsets, &mut hashes, 0..line_count);

        Self {
            buffer_id,
            version,
            language,
            engine,
            status,
            duration_ms: 0,
            line_start_bytes: offsets,
            line_hashes: hashes,
            per_line,
            highlighted_range: None,
        }
    }
}
