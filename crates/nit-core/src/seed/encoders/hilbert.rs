use nit_utils::hashing::stable_hash_bytes;
use nit_utils::rng::SplitMix64;

use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::hilbert_index_to_xy;
use crate::seed::view_modes::SeedEncoderId;

pub(super) struct HilbertBitsEncoder;

impl SeedEncoder for HilbertBitsEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::HilbertBits
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let len = bytes.len();
        let mut rng =
            SplitMix64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64) ^ 0x5eed_u64);
        for idx in 0..size * size {
            let (x, y) = hilbert_index_to_xy(order, idx as u32);
            let base = if len == 0 { 0 } else { bytes[idx % len] };
            let mix = (rng.next_u64() & 0xff) as u8;
            let value = base ^ mix;
            grid.set(x as usize, y as usize, value);
        }
        grid
    }
}
