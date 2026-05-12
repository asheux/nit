//! Node-class shim for the structural encoder.
//!
//! `ast_node_class` (the 0-255 kind weight used by AstStructureEncoder)
//! lives in [`crate::seed::encoders::node_class`]. This module re-exports
//! it and adds a small `kind_band_label` helper used by per-encoder
//! diagnostics — turning a weight into a coarse band string so log lines
//! and structural-encoder summaries can describe their grids without
//! depending on the encoder-wide `RoleBand` enum.

#[allow(unused_imports)]
pub(super) use crate::seed::encoders::node_class::ast_node_class;

/// Coarse label for a `kind_weight` band, useful in diagnostic summaries.
/// The bands match the buckets used by `ast_node_class` itself.
#[allow(dead_code)]
pub(super) fn kind_band_label(weight: u8) -> &'static str {
    match weight {
        250..=255 => "declaration",
        200..=249 => "control_flow",
        160..=199 => "expression",
        120..=159 => "statement",
        70..=119 => "type",
        30..=69 => "literal",
        _ => "other",
    }
}

#[allow(dead_code)]
pub(super) fn is_declaration(weight: u8) -> bool {
    weight >= 250
}

#[allow(dead_code)]
pub(super) fn is_control_flow(weight: u8) -> bool {
    (200..=249).contains(&weight)
}
