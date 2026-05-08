use nit_utils::hashing::stable_hash_bytes;
use nit_utils::rng::SplitMix64;

use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::view_modes::SeedEncoderId;

pub(super) struct Lifehash16Encoder;

impl SeedEncoder for Lifehash16Encoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::Lifehash16
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 16usize;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let mut rng =
            SplitMix64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64) ^ 0x16_u64);
        for idx in 0..size * size {
            let value = (rng.next_u64() & 0xff) as u8;
            let x = idx % size;
            let y = idx / size;
            grid.set(x, y, value);
        }
        grid
    }
}
