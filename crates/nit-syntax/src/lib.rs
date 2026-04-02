//! Syntax highlighting engine using tree-sitter grammars.
//!
//! Provides incremental, viewport-aware syntax highlighting for source code
//! buffers. Supports multiple languages via tree-sitter grammars and falls
//! back to plain text when a grammar is unavailable.
//!
//! # Architecture
//!
//! ```text
//!   Editor
//!     |  HighlightRequest
//!     v
//!  SyntaxManager -- TreeSitterEngine (background thread)
//!                `- PlainTextEngine  (inline fallback)
//!     |
//!     v
//!  HighlightSnapshot -> per-line LineSegments -> renderer
//! ```
//!
//! The primary entry point is [`SyntaxManager`], which owns both engine
//! backends and routes requests based on [`SyntaxConfig`].
//!
//! # Debouncing
//!
//! Use [`Debouncer`] to rate-limit rehighlight requests so rapid edits
//! (e.g. typing) do not flood the tree-sitter worker thread.
//!
//! # Language detection
//!
//! [`LanguageRegistry`] identifies languages from file extensions, shebang
//! lines, or explicit overrides, then provides the matching tree-sitter
//! grammar and highlight queries.

#![forbid(unsafe_code)]

use std::fmt;

// ── Internal modules ───────────────────────────────────────────────────────

mod captures;
mod debounce;
mod engine;
mod highlight;
mod registry;
mod tree_sitter_engine;

// ── Debounce ──────────────────────────────────────────────────────────────

/// Rate-limiter for throttling rehighlight requests during rapid edits.
pub use debounce::{Debouncer, DebouncerPhase};

// ── Engine layer ──────────────────────────────────────────────────────────

/// Configuration, trait definitions, and multiplexing engine implementations.
pub use engine::{
    HighlightRequest, PlainTextEngine, SyntaxConfig, SyntaxEngine, SyntaxManager, ViewportRange,
};

// ── Highlight output ──────────────────────────────────────────────────────

/// Span types, per-line segments, snapshots, and byte-to-char mapping.
pub use highlight::{
    hash_line_bytes, map_line_segments_to_chars, EngineKind, HighlightGroup, HighlightSnapshot,
    HighlightSpan, LineSegment, MappedLineSegment, SegmentMapError, SyntaxStatus,
};

// ── Language registry ─────────────────────────────────────────────────────

/// Language identification from paths and shebangs, plus grammar lookup.
pub use registry::{LanguageId, LanguageRegistry};

// ── Capture categories ────────────────────────────────────────────────────

/// Semantic grouping of capture families for theme-level operations.
pub use captures::{capture_entry_count, CaptureCategory, CATEGORY_COUNT};

// ── Constants ─────────────────────────────────────────────────────────────

/// Maximum source-file size (in bytes) that the highlighting engine will
/// fully process via tree-sitter. Files larger than this threshold fall
/// back to [`PlainTextEngine`] to avoid excessive memory pressure.
pub const MAX_HIGHLIGHT_BYTES: usize = 5 * 1024 * 1024;

// ── Highlight outcome ─────────────────────────────────────────────────────

/// Summary of a highlight pass — describes what happened during syntax
/// analysis of a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HighlightOutcome {
    /// Full tree-sitter parse completed successfully.
    Parsed,
    /// Viewport-scoped partial parse completed.
    ViewportOnly,
    /// Fell back to plain-text (no grammar or file too large).
    PlainText,
}

// ── HighlightOutcome — string conversion ──────────────────────────────────

impl HighlightOutcome {
    /// Human-readable label for this outcome, returned as a static slice.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Parsed => "parsed",
            Self::ViewportOnly => "viewport-only",
            Self::PlainText => "plain-text",
        }
    }
}

impl fmt::Display for HighlightOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ── HighlightOutcome — classification ─────────────────────────────────────

impl HighlightOutcome {
    /// Returns `true` when the tree-sitter engine produced the result
    /// (either full or viewport-scoped parse).
    pub fn is_tree_sitter(self) -> bool {
        matches!(self, Self::Parsed | Self::ViewportOnly)
    }

    /// Convert from an [`EngineKind`] and a viewport flag into the
    /// appropriate outcome variant.
    pub fn from_engine(engine_kind: EngineKind, viewport_scoped: bool) -> Self {
        match engine_kind {
            EngineKind::TreeSitter if viewport_scoped => Self::ViewportOnly,
            EngineKind::TreeSitter => Self::Parsed,
            EngineKind::Plain => Self::PlainText,
        }
    }
}

// ── File classification ───────────────────────────────────────────────────

/// Classification of a source buffer for highlighting strategy selection.
///
/// Determines whether a file should receive full tree-sitter parsing,
/// viewport-only parsing, or be skipped entirely based on byte length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileClassification {
    /// Source is within normal limits for full tree-sitter highlighting.
    Normal,
    /// Source exceeds [`MAX_HIGHLIGHT_BYTES`]; viewport-only mode recommended.
    Oversized,
    /// Source buffer is empty — no highlighting work needed.
    Empty,
}

// ── FileClassification — construction ─────────────────────────────────────

impl FileClassification {
    /// Classify a source buffer by its byte length.
    ///
    /// Returns [`Empty`](Self::Empty) for zero-length buffers,
    /// [`Oversized`](Self::Oversized) above the byte limit, and
    /// [`Normal`](Self::Normal) otherwise.
    pub fn from_byte_length(total_bytes: usize) -> Self {
        if total_bytes == 0 {
            Self::Empty
        } else if total_bytes > MAX_HIGHLIGHT_BYTES {
            Self::Oversized
        } else {
            Self::Normal
        }
    }

    /// Whether this classification allows full tree-sitter highlighting.
    pub fn supports_full_highlight(self) -> bool {
        matches!(self, Self::Normal)
    }
}

// ── FileClassification — outcome prediction ───────────────────────────────

impl FileClassification {
    /// The expected [`HighlightOutcome`] given this file classification
    /// and whether viewport-scoped parsing is enabled.
    pub fn expected_outcome(self, viewport_enabled: bool) -> HighlightOutcome {
        match self {
            Self::Normal if viewport_enabled => HighlightOutcome::ViewportOnly,
            Self::Normal => HighlightOutcome::Parsed,
            Self::Oversized | Self::Empty => HighlightOutcome::PlainText,
        }
    }
}

impl fmt::Display for FileClassification {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self {
            Self::Normal => "normal (full parse)",
            Self::Oversized => "oversized (viewport-only)",
            Self::Empty => "empty (no-op)",
        };
        output.write_str(description)
    }
}

// ── Categorizable trait ───────────────────────────────────────────────────

/// Trait for highlight types that can report their semantic category.
///
/// Implemented by [`HighlightGroup`] to enable category-level filtering
/// and theming operations without pattern matching at each call site.
pub trait Categorizable {
    /// The semantic [`CaptureCategory`] this entity belongs to.
    fn category(&self) -> CaptureCategory;

    /// Whether this entity belongs to the given target category.
    fn belongs_to(&self, target_category: CaptureCategory) -> bool {
        self.category() == target_category
    }
}

/// Blanket categorization for [`HighlightGroup`] — the most commonly
/// classified type in the highlighting pipeline.
impl Categorizable for HighlightGroup {
    fn category(&self) -> CaptureCategory {
        CaptureCategory::of_group(*self)
    }
}

// ── Test suite ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
