/// BLAKE3 digest truncated to 64 bits (little-endian).
#[inline]
#[must_use]
pub fn stable_hash_bytes(data: &[u8]) -> u64 {
    let digest = blake3::hash(data);
    let bytes: [u8; 8] = digest.as_bytes()[..8]
        .try_into()
        .expect("blake3 digest is 32 bytes");
    u64::from_le_bytes(bytes)
}

/// Not suitable for cryptographic use.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Golden-ratio-derived additive constant for the state update.
    const INCREMENT: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Avoids degenerate all-zeros output from a zero seed.
    const ZERO_GUARD: u64 = 0x4d59_5df4_d0f3_3173;

    #[must_use]
    pub fn new(seed: u64) -> Self {
        let state = if seed == 0 { Self::ZERO_GUARD } else { seed };
        Self { state }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(Self::INCREMENT);
        stafford_mix13(self.state)
    }

    /// Uses rejection sampling to avoid modulo bias.
    pub fn next_bounded(&mut self, upper: u64) -> u64 {
        if upper <= 1 {
            return 0;
        }
        let threshold = upper.wrapping_neg() % upper;
        let mut candidate = self.next_u64();
        while candidate < threshold {
            candidate = self.next_u64();
        }
        candidate % upper
    }

    /// Draws from the upper 24 mantissa bits for uniform `[0.0, 1.0)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

impl Iterator for SplitMix64 {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        Some(self.next_u64())
    }
}

#[inline]
const fn stafford_mix13(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
