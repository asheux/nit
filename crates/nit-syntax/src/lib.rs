//! Incremental, viewport-aware syntax highlighting via tree-sitter.
//!
//! [`SyntaxManager`] is the top-level entry point; see
//! [`HighlightRequest`] for the input shape and [`HighlightSnapshot`]
//! for the output.

#![forbid(unsafe_code)]

use std::fmt;

mod captures;
mod debounce;
mod engine;
mod highlight;
mod registry;
mod tree_sitter_engine;

pub use captures::{capture_entry_count, CaptureCategory, Categorizable, CATEGORY_COUNT};
pub use debounce::{Debouncer, DebouncerPhase};
pub use engine::{
    HighlightRequest, PlainTextEngine, SyntaxConfig, SyntaxEngine, SyntaxManager, ViewportRange,
};
pub use highlight::{
    hash_line_bytes, map_line_segments_to_chars, EngineKind, HighlightGroup, HighlightSnapshot,
    HighlightSpan, LineSegment, MappedLineSegment, SegmentMapError, SyntaxStatus,
};
pub use registry::{LanguageId, LanguageRegistry};

/// Files larger than this are treated as oversized and bypass full parsing.
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

    #[must_use]
    pub const fn from_engine(engine: EngineKind, viewport_scoped: bool) -> Self {
        match engine {
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
    #[must_use]
    pub const fn from_byte_length(byte_len: usize) -> Self {
        match byte_len {
            0 => Self::Empty,
            n if n > MAX_HIGHLIGHT_BYTES => Self::Oversized,
            _ => Self::Normal,
        }
    }

    #[must_use]
    pub const fn supports_full_highlight(self) -> bool {
        matches!(self, Self::Normal)
    }

    #[must_use]
    pub const fn expected_outcome(self, viewport_scoped: bool) -> HighlightOutcome {
        match self {
            Self::Normal if viewport_scoped => HighlightOutcome::ViewportOnly,
            Self::Normal => HighlightOutcome::Parsed,
            Self::Oversized | Self::Empty => HighlightOutcome::PlainText,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal (full parse)",
            Self::Oversized => "oversized (viewport-only)",
            Self::Empty => "empty (no-op)",
        }
    }
}

impl fmt::Display for FileClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
