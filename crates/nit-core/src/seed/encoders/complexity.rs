//! Complexity field encoder. Split into:
//! - [`encoder`] — the public `ComplexityFieldEncoder` and chunk routing.
//! - [`metrics`] — single-pass chunk_metrics aggregator (nesting +
//!   cognitive + entropy + diversity) over an AST node window.
//!
//! Chunk-by-node-index keeps the grid stable when comments shift source
//! rows. SonarSource-style cognitive complexity penalises nested ladders
//! that plain cyclomatic treats as linear branch count.

#[path = "complexity/encoder.rs"]
mod encoder;
#[path = "complexity/metrics.rs"]
mod metrics;

pub(crate) use encoder::ComplexityFieldEncoder;
