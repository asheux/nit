use std::cmp::Ordering;

use crate::registry::LanguageId;

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EngineKind {
    TreeSitter,
    Plain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyntaxStatus {
    Ok(EngineKind),
    Disabled,
    LargeFile,
    Error(String),
}

impl SyntaxStatus {
    pub fn label(&self) -> String {
        match self {
            SyntaxStatus::Ok(EngineKind::TreeSitter) => "TS(ok)".to_string(),
            SyntaxStatus::Ok(EngineKind::Plain) => "Plain(ok)".to_string(),
            SyntaxStatus::Disabled => "Off".to_string(),
            SyntaxStatus::LargeFile => "Plain(large)".to_string(),
            SyntaxStatus::Error(_) => "TS(error)".to_string(),
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
        let line_start_bytes = compute_line_starts(text);
        let line_hashes = compute_line_hashes(text, &line_start_bytes);
        let per_line = vec![Vec::new(); line_start_bytes.len().saturating_sub(1)];
        HighlightSnapshot {
            buffer_id,
            version,
            language,
            engine,
            status,
            duration_ms: 0,
            line_start_bytes,
            line_hashes,
            per_line,
        }
    }

    pub fn from_spans(
        buffer_id: usize,
        version: u64,
        language: LanguageId,
        engine: EngineKind,
        status: SyntaxStatus,
        text: &str,
        mut spans: Vec<HighlightSpan>,
        max_spans_per_line: usize,
    ) -> Self {
        // Most engines already emit spans in source order; avoid an O(n log n) sort when possible.
        // Required order: start_byte ASC, priority DESC for ties.
        let mut needs_sort = false;
        for pair in spans.windows(2) {
            let a = &pair[0];
            let b = &pair[1];
            match a.start_byte.cmp(&b.start_byte) {
                Ordering::Less => {}
                Ordering::Equal => {
                    if a.priority < b.priority {
                        needs_sort = true;
                        break;
                    }
                }
                Ordering::Greater => {
                    needs_sort = true;
                    break;
                }
            }
        }
        if needs_sort {
            spans.sort_by(|a, b| match a.start_byte.cmp(&b.start_byte) {
                Ordering::Equal => b.priority.cmp(&a.priority),
                other => other,
            });
        }
        let line_start_bytes = compute_line_starts(text);
        let mut per_line = vec![Vec::new(); line_start_bytes.len().saturating_sub(1)];

        for span in spans {
            if span.end_byte <= span.start_byte {
                continue;
            }
            let start_line = find_line(&line_start_bytes, span.start_byte);
            let end_line = find_line(&line_start_bytes, span.end_byte);
            for line in start_line..=end_line {
                if line + 1 >= line_start_bytes.len() {
                    continue;
                }
                if max_spans_per_line > 0 && per_line[line].len() >= max_spans_per_line {
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

        let line_hashes = compute_line_hashes(text, &line_start_bytes);
        HighlightSnapshot {
            buffer_id,
            version,
            language,
            engine,
            status,
            duration_ms: 0,
            line_start_bytes,
            line_hashes,
            per_line,
        }
    }
}

pub fn hash_line_bytes(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut end = bytes.len();
    if end > 0 && bytes[end - 1] == b'\n' {
        end = end.saturating_sub(1);
    }
    let mut hash = OFFSET;
    for &b in &bytes[..end] {
        if b == b'\r' {
            continue;
        }
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn compute_line_hashes(text: &str, line_starts: &[usize]) -> Vec<u64> {
    let bytes = text.as_bytes();
    let mut hashes = Vec::with_capacity(line_starts.len().saturating_sub(1));
    for idx in 0..line_starts.len().saturating_sub(1) {
        let start = line_starts[idx];
        let end = line_starts[idx + 1];
        hashes.push(hash_line_bytes(&bytes[start..end]));
    }
    hashes
}

pub fn map_line_segments_to_chars(
    line: &str,
    segments: &[LineSegment],
) -> Result<Vec<MappedLineSegment>, SegmentMapError> {
    if segments.is_empty() || line.is_empty() {
        return Ok(Vec::new());
    }
    let line_len = line.len();
    let mut boundaries = Vec::with_capacity(line.chars().count().saturating_add(1));
    for (idx, _) in line.char_indices() {
        boundaries.push(idx);
    }
    boundaries.push(line_len);

    let mut mapped = Vec::with_capacity(segments.len());
    for seg in segments {
        let start = seg.start;
        let mut end = seg.end;
        if start >= line_len {
            continue;
        }
        if end > line_len {
            end = line_len;
        }
        if start >= end {
            continue;
        }
        let start_idx = boundaries
            .binary_search(&start)
            .map_err(|_| SegmentMapError {
                start,
                end,
                line_len,
            })?;
        let end_idx = boundaries
            .binary_search(&end)
            .map_err(|_| SegmentMapError {
                start,
                end,
                line_len,
            })?;
        if start_idx >= end_idx {
            continue;
        }
        mapped.push(MappedLineSegment {
            start: start_idx,
            end: end_idx,
            group: seg.group,
        });
    }
    Ok(mapped)
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
