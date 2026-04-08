//! Deterministic hashing and pseudo-random number generation.

use blake3::Hasher;

/// Deterministic 64-bit hash of `data` via BLAKE3 (little-endian first 8 bytes).
#[must_use]
pub fn stable_hash_bytes(data: &[u8]) -> u64 {
    let mut hasher = Hasher::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(buf)
}

/// Golden-ratio-derived additive constant for the SplitMix64 state update.
const INCREMENT: u64 = 0x9E37_79B9_7F4A_7C15;

/// Replacement seed when the caller supplies zero, avoiding degenerate output.
const ZERO_GUARD: u64 = 0x4d59_5df4_d0f3_3173;

/// SplitMix64 pseudo-random number generator for non-cryptographic use.
///
/// Implements `Iterator<Item = u64>` for convenient streaming. The sequence
/// is fully deterministic given the same seed.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Creates a new generator. Zero seeds are replaced to avoid degeneracy.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let initial = if seed == 0 { ZERO_GUARD } else { seed };
        Self { state: initial }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(INCREMENT);
        avalanche_mix(self.state)
    }

    /// Returns a value in `[0, upper)` using rejection sampling to avoid modulo bias.
    pub fn next_bounded(&mut self, upper: u64) -> u64 {
        if upper <= 1 {
            return 0;
        }
        let threshold = upper.wrapping_neg() % upper;
        loop {
            let candidate = self.next_u64();
            if candidate >= threshold {
                return candidate % upper;
            }
        }
    }

    /// Pseudo-random `f32` in `[0.0, 1.0)` from the upper 24 mantissa bits.
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

/// Stafford Mix13 avalanche — spreads entropy across all 64 bits.
#[inline]
fn avalanche_mix(raw: u64) -> u64 {
    let mixed = (raw ^ (raw >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let refined = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    refined ^ (refined >> 31)
}
