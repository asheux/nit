//! Seed encoders: source text → 32×32 GoL genome grid. Each encoder captures a
//! different facet of program structure (AST shape, token spectrum, complexity
//! field). The pipeline is `encode_seed(input, encoder, params, …)` →
//! deterministic `EncodedSeed` containing the value grid, packed bits, and
//! resulting GoL `Grid` plus stats.

pub(crate) mod encoders;
mod grid_types;
mod params;
mod utils;
mod view_modes;

pub use encoders::encode_seed;

#[cfg(test)]
pub(crate) use encoders::{AstStructureEncoder, ComplexityFieldEncoder, TokenSpectrumEncoder};
pub use grid_types::{EncodedSeed, SeedBits, SeedEncoder, SeedInput, SeedStats, SeedValueGrid};
pub use params::{SeedParams, SeedPlacement, SeedSymmetry};
pub use view_modes::{SeedEncoderId, SeedPreviewMode, SeedViewMode};

#[cfg(test)]
#[path = "../tests/seed.rs"]
mod tests;
