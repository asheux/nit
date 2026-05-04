use std::fmt;

pub use crate::rng::SplitMix64;

/// BLAKE3 digest truncated to 64 bits (little-endian).
#[must_use]
pub fn stable_hash_bytes(data: &[u8]) -> u64 {
    let digest = blake3::hash(data);
    let prefix = *digest
        .as_bytes()
        .first_chunk::<8>()
        .expect("blake3 digest is 32 bytes");
    u64::from_le_bytes(prefix)
}

/// Consistent across runs and platforms.
pub trait Fingerprint {
    #[must_use]
    fn fingerprint(&self) -> u64;
}

impl<T: AsRef<[u8]> + ?Sized> Fingerprint for T {
    #[inline]
    fn fingerprint(&self) -> u64 {
        stable_hash_bytes(self.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentTag {
    pub prefix: String,
    pub digest: u32,
}

impl ContentTag {
    #[must_use]
    pub fn new(prefix: &str, payload: &[u8]) -> Self {
        let digest = stable_hash_bytes(payload) as u32;
        Self {
            prefix: prefix.to_owned(),
            digest,
        }
    }
}

impl fmt::Display for ContentTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:08x}", self.prefix, self.digest)
    }
}
