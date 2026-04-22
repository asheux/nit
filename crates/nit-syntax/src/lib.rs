//! Incremental, viewport-aware syntax highlighting via tree-sitter.
//!
//! [`SyntaxManager`] is the top-level entry point; see
//! [`HighlightRequest`] for the input shape and [`HighlightSnapshot`]
//! for the output.

#![forbid(unsafe_code)]

mod captures;
mod classification;
mod engine;
mod highlight;
mod language;

pub use captures::{capture_entry_count, CaptureCategory, Categorizable, CATEGORY_COUNT};
pub use classification::{FileClassification, HighlightOutcome, MAX_HIGHLIGHT_BYTES};
pub use engine::{
    HighlightRequest, PlainTextEngine, SyntaxConfig, SyntaxEngine, SyntaxManager, ViewportRange,
};
pub use highlight::{
    hash_line_bytes, map_line_segments_to_chars, EngineKind, HighlightGroup, HighlightSnapshot,
    HighlightSpan, LineSegment, MappedLineSegment, SegmentMapError, SyntaxStatus,
};
pub use language::{LanguageId, LanguageRegistry};
pub use nit_utils::debounce::{Debouncer, DebouncerPhase};

#[cfg(test)]
mod tests;
