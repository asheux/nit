//! Companion reference for the structural encoder. NOT loaded as a Rust
//! module — `encoders/mod.rs` declares the encoder via
//! `#[path = "structural.rs"] mod structural;`. This sibling file holds
//! the tuning constants that describe the four-channel weighted projection
//! so a future tuning pass can read one place rather than digging in
//! `encoder.rs` / `aggregations.rs`.

#![allow(dead_code)]

pub(crate) const STRUCTURAL_GRID_ORDER: u32 = 5;
pub(crate) const STRUCTURAL_GRID_SIZE: usize = 1 << STRUCTURAL_GRID_ORDER;

pub(crate) const DIVERSITY_WEIGHT: f32 = 0.35;
pub(crate) const DEPTH_WEIGHT: f32 = 0.25;
pub(crate) const ENTROPY_WEIGHT: f32 = 0.20;
pub(crate) const UNIQUENESS_WEIGHT: f32 = 0.20;

pub(crate) const NGRAM_WINDOW: usize = 4;
pub(crate) const NGRAM_SEARCH: usize = 256;

pub(crate) const ENCODER_LABEL: &str = "structural";

pub(crate) struct StructuralTunables {
    pub grid_order: u32,
    pub diversity: f32,
    pub depth: f32,
    pub entropy: f32,
    pub uniqueness: f32,
    pub ngram_window: usize,
    pub ngram_search: usize,
}

pub(crate) const DEFAULT_TUNABLES: StructuralTunables = StructuralTunables {
    grid_order: STRUCTURAL_GRID_ORDER,
    diversity: DIVERSITY_WEIGHT,
    depth: DEPTH_WEIGHT,
    entropy: ENTROPY_WEIGHT,
    uniqueness: UNIQUENESS_WEIGHT,
    ngram_window: NGRAM_WINDOW,
    ngram_search: NGRAM_SEARCH,
};
