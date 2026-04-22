//! Highlight output types and the span → line distribution pipeline.

mod chars;
mod lines;
mod spans;
mod types;

pub use chars::map_line_segments_to_chars;
pub use lines::hash_line_bytes;
pub use types::{
    EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan, LineSegment, MappedLineSegment,
    SegmentMapError, SyntaxStatus,
};

pub(crate) use lines::{compute_line_starts, find_line, recompute_line_hashes};
pub(crate) use spans::{distribute_spans_to_lines, sort_spans};
