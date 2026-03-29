//! Syntax highlighting engine using tree-sitter grammars.

#![forbid(unsafe_code)]

mod debounce;
mod engine;
mod highlight;
mod registry;
mod tree_sitter_engine;

pub use debounce::Debouncer;
pub use engine::{
    HighlightRequest, PlainTextEngine, SyntaxConfig, SyntaxEngine, SyntaxManager, ViewportRange,
};
pub use highlight::{
    hash_line_bytes, map_line_segments_to_chars, EngineKind, HighlightGroup, HighlightSnapshot,
    HighlightSpan, LineSegment, MappedLineSegment, SegmentMapError, SyntaxStatus,
};
pub use registry::{LanguageId, LanguageRegistry};

#[cfg(test)]
mod tests;
