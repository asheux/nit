//! Structural encoder. Split into:
//! - [`encoder`] — the public `StructuralEncoder` impl and `tokens_from_features`.
//! - [`aggregations`] — per-window role diversity, depth gradient, role
//!   entropy, role n-gram uniqueness.
//! - [`language`] — per-encoder language-detection re-exports and diagnostic
//!   helpers used by structural-summary log lines.
//! - [`node_class`] — `ast_node_class` re-export plus weight-band label
//!   helpers used by structural diagnostics.
//!
//! Channel weights live in `encoder.rs` (35 / 25 / 20 / 20 across
//! diversity / depth / entropy / uniqueness). Varied structure yields rich
//! GoL genomes; uniform code yields flat grids that die quickly.

#[path = "structural/aggregations.rs"]
mod aggregations;
#[path = "structural/encoder.rs"]
mod encoder;
#[path = "structural/language.rs"]
mod language;
#[path = "structural/node_class.rs"]
mod node_class;

pub(crate) use encoder::StructuralEncoder;
