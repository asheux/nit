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
            SyntaxStatus::Ok(EngineKind::TreeSitter) => "TS (ok)".to_string(),
            SyntaxStatus::Ok(EngineKind::Plain) => "Plain (ok)".to_string(),
            SyntaxStatus::Disabled => "Off".to_string(),
            SyntaxStatus::LargeFile => "Plain (large)".to_string(),
            SyntaxStatus::Error(_) => "Error".to_string(),
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

#[derive(Clone, Debug)]
pub struct HighlightSnapshot {
    pub buffer_id: usize,
    pub version: u64,
    pub language: LanguageId,
    pub engine: EngineKind,
    pub status: SyntaxStatus,
    pub line_start_bytes: Vec<usize>,
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
        let per_line = vec![Vec::new(); line_start_bytes.len().saturating_sub(1)];
        HighlightSnapshot {
            buffer_id,
            version,
            language,
            engine,
            status,
            line_start_bytes,
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
        spans.sort_by(|a, b| match a.start_byte.cmp(&b.start_byte) {
            Ordering::Equal => b.priority.cmp(&a.priority),
            other => other,
        });
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

        HighlightSnapshot {
            buffer_id,
            version,
            language,
            engine,
            status,
            line_start_bytes,
            per_line,
        }
    }
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
