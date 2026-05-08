use nit_utils::hashing::stable_hash_bytes;
use nit_utils::rng::SplitMix64;

use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::view_modes::SeedEncoderId;

pub(super) struct AsciiBytesEncoder;

impl SeedEncoder for AsciiBytesEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::AsciiBytes
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 32usize;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let mut rng = SplitMix64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64));
        let len = bytes.len();
        for idx in 0..size * size {
            let base = if len == 0 { 0 } else { bytes[idx % len] };
            let mix = (rng.next_u64() & 0xff) as u8;
            let value = base.wrapping_add((idx as u8).wrapping_mul(31)) ^ mix;
            let x = idx % size;
            let y = idx / size;
            grid.set(x, y, value);
        }
        grid
    }
}
