//! Incremental, viewport-aware syntax highlighting via tree-sitter.

#![forbid(unsafe_code)]

use std::fmt;

mod captures;
mod debounce;
mod engine;
mod highlight;
mod registry;
mod tree_sitter_engine;

pub use captures::{capture_entry_count, CaptureCategory, CATEGORY_COUNT};
pub use debounce::{Debouncer, DebouncerPhase};
pub use engine::{
    HighlightRequest, PlainTextEngine, SyntaxConfig, SyntaxEngine, SyntaxManager, ViewportRange,
};
pub use highlight::{
    hash_line_bytes, map_line_segments_to_chars, EngineKind, HighlightGroup, HighlightSnapshot,
    HighlightSpan, LineSegment, MappedLineSegment, SegmentMapError, SyntaxStatus,
};
pub use registry::{LanguageId, LanguageRegistry};

pub const MAX_HIGHLIGHT_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HighlightOutcome {
    Parsed,
    ViewportOnly,
    PlainText,
}

impl HighlightOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Parsed => "parsed",
            Self::ViewportOnly => "viewport-only",
            Self::PlainText => "plain-text",
        }
    }

    pub const fn is_tree_sitter(self) -> bool {
        matches!(self, Self::Parsed | Self::ViewportOnly)
    }

    pub const fn from_engine(engine_kind: EngineKind, viewport_scoped: bool) -> Self {
        match engine_kind {
            EngineKind::TreeSitter if viewport_scoped => Self::ViewportOnly,
            EngineKind::TreeSitter => Self::Parsed,
            EngineKind::Plain => Self::PlainText,
        }
    }
}

impl fmt::Display for HighlightOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileClassification {
    Normal,
    Oversized,
    Empty,
}

impl FileClassification {
    pub const fn from_byte_length(total_bytes: usize) -> Self {
        if total_bytes == 0 {
            return Self::Empty;
        }
        if total_bytes > MAX_HIGHLIGHT_BYTES {
            return Self::Oversized;
        }
        Self::Normal
    }

    pub const fn supports_full_highlight(self) -> bool {
        matches!(self, Self::Normal)
    }

    pub const fn expected_outcome(self, viewport_scoped: bool) -> HighlightOutcome {
        match self {
            Self::Normal if viewport_scoped => HighlightOutcome::ViewportOnly,
            Self::Normal => HighlightOutcome::Parsed,
            Self::Oversized | Self::Empty => HighlightOutcome::PlainText,
        }
    }
}

impl fmt::Display for FileClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "normal (full parse)",
            Self::Oversized => "oversized (viewport-only)",
            Self::Empty => "empty (no-op)",
        })
    }
}

pub trait Categorizable {
    fn category(&self) -> CaptureCategory;

    fn belongs_to(&self, target_category: CaptureCategory) -> bool {
        self.category() == target_category
    }
}

impl Categorizable for HighlightGroup {
    fn category(&self) -> CaptureCategory {
        CaptureCategory::of_group(*self)
    }
}

#[cfg(test)]
mod tests;
