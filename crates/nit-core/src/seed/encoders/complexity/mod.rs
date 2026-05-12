//! Companion reference for the complexity field encoder. NOT loaded as a
//! Rust module — `encoders/mod.rs` declares the encoder via
//! `#[path = "complexity.rs"] mod complexity;`. This sibling file holds
//! the constants documenting the encoder's tuning so a future tuning pass
//! can read one place instead of fishing in `encoder.rs` and `metrics.rs`.

#![allow(dead_code)]

pub(crate) const COMPLEXITY_GRID_SIZE: usize = 32;
pub(crate) const NESTING_WEIGHT: f32 = 0.25;
pub(crate) const COGNITIVE_WEIGHT: f32 = 0.30;
pub(crate) const ENTROPY_WEIGHT: f32 = 0.25;
pub(crate) const DIVERSITY_WEIGHT: f32 = 0.20;
pub(crate) const COGNITIVE_CAP: u32 = 36;
pub(crate) const MAX_NESTING_DEPTH: u8 = 15;
pub(crate) const VALUE_MAX: f32 = 255.0;

pub(crate) const ENCODER_LABEL: &str = "complexity_field";

pub(crate) struct ComplexityTunables {
    pub grid_size: usize,
    pub nesting_weight: f32,
    pub cognitive_weight: f32,
    pub entropy_weight: f32,
    pub diversity_weight: f32,
    pub cognitive_cap: u32,
}

pub(crate) const DEFAULT_TUNABLES: ComplexityTunables = ComplexityTunables {
    grid_size: COMPLEXITY_GRID_SIZE,
    nesting_weight: NESTING_WEIGHT,
    cognitive_weight: COGNITIVE_WEIGHT,
    entropy_weight: ENTROPY_WEIGHT,
    diversity_weight: DIVERSITY_WEIGHT,
    cognitive_cap: COGNITIVE_CAP,
};
