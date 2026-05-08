use std::path::Path;

use nit_gol::Grid;
use serde::{Deserialize, Serialize};

use crate::config::GolSeedSource;

use super::params::SeedParams;
use super::view_modes::SeedEncoderId;

pub struct SeedInput<'a> {
    pub text: &'a str,
    pub source: GolSeedSource,
    pub file_path: Option<&'a Path>,
    pub version: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SeedStats {
    pub density: f32,
    pub alive: usize,
    pub components: usize,
    pub base_width: usize,
    pub base_height: usize,
}

// Generic 2D grid. Methods (and the bounds-checked accessors) live on the
// impl block in `super::utils` so the type definitions here stay free of
// trivial accessor noise. Fields are pub(super) so the impl block can
// construct/access them while keeping the API surface read-only externally.
#[derive(Clone, Debug)]
pub struct Grid2D<T: Copy + Default> {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) data: Vec<T>,
}

pub type SeedValueGrid = Grid2D<u8>;

// SeedBits is a separate struct (not a Grid2D alias) because its public API
// uses `bool` while internal storage is u8 (one byte per bit, kept that way
// so `cells` can be hashed as raw bytes). Methods live on the impl block in
// `super::utils`; fields are pub(super) so that block can construct/access
// them while keeping the API surface read-only externally.
#[derive(Clone, Debug)]
pub struct SeedBits {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) cells: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct EncodedSeed {
    pub encoder_id: SeedEncoderId,
    pub params: SeedParams,
    pub variant: u8,
    pub input_hash: u64,
    pub seed_hash: u64,
    pub source: GolSeedSource,
    pub base_values: SeedValueGrid,
    pub base_bits: SeedBits,
    pub base_bits_raw: SeedBits,
    pub grid: Grid,
    pub stats: SeedStats,
}

pub trait SeedEncoder {
    fn id(&self) -> SeedEncoderId;
    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid;
}
