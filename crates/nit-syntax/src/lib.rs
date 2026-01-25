#![forbid(unsafe_code)]

mod debounce;
mod engine;
mod highlight;
mod registry;
mod tree_sitter_engine;

pub use debounce::Debouncer;
pub use engine::{HighlightRequest, PlainTextEngine, SyntaxConfig, SyntaxEngine, SyntaxManager};
pub use highlight::{
    EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan, LineSegment, SyntaxStatus,
};
pub use registry::{LanguageId, LanguageRegistry};

#[cfg(test)]
mod tests;
