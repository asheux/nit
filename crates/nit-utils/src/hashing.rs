//! Deterministic hashing and fast pseudo-random number generation.
//!
//! [`stable_hash_bytes`] produces a deterministic 64-bit hash from arbitrary
//! byte slices using BLAKE3, suitable for content-addressing and deduplication.
//!
//! [`SplitMix64`] is a lightweight, seedable PRNG for cases where cryptographic
//! security is unnecessary — simulations, shuffling, and procedural generation.

use blake3::Hasher;

/// Computes a deterministic 64-bit hash of `data` using BLAKE3.
///
/// The first 8 bytes of the BLAKE3 digest are interpreted as a little-endian
/// `u64`. The result is stable across runs and platforms, making it suitable
/// for content-addressing, deduplication keys, and reproducible seeding.
///
/// # Examples
///
/// ```
/// let h = nit_utils::hashing::stable_hash_bytes(b"hello");
/// assert_ne!(h, 0);
/// ```
#[inline]
#[must_use]
pub fn stable_hash_bytes(data: &[u8]) -> u64 {
    let mut hasher = Hasher::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(out)
}

/// A fast, seedable 64-bit pseudo-random number generator.
///
/// Implements the SplitMix64 algorithm, which provides excellent statistical
/// quality for non-cryptographic use cases such as simulations, game logic, and
/// reproducible test fixtures.
///
/// Seed `0` is mapped to a built-in constant to avoid the degenerate all-zeros
/// state.
///
/// # Examples
///
/// ```
/// let mut rng = nit_utils::hashing::SplitMix64::new(42);
/// let value = rng.next_u64();
/// assert_ne!(value, 0);
/// ```
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Internal fallback used when the caller supplies a zero seed, ensuring
    /// the generator never starts in the degenerate all-zeros state.
    const FALLBACK_SEED: u64 = 0x4d59_5df4_d0f3_3173;

    /// Creates a new generator seeded with `seed`.
    ///
    /// A seed of `0` is replaced by an internal constant to guarantee the
    /// generator never enters the all-zeros state.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let state = if seed == 0 { Self::FALLBACK_SEED } else { seed };
        Self { state }
    }

    /// Advances the internal state and returns the next pseudo-random `u64`.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a pseudo-random `f32` in the half-open range `[0.0, 1.0)`.
    ///
    /// Uses the upper 24 bits of a `u64` draw to fill the mantissa, giving
    /// uniform distribution over representable single-precision floats in
    /// that range.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}
