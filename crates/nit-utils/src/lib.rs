#![forbid(unsafe_code)]

use std::fmt;

pub mod fs;
pub mod hashing;
pub mod paths;
pub mod time;

pub use fs::{ensure_dir, write_atomic};
pub use hashing::{stable_hash_bytes, SplitMix64};

/// Deterministic 64-bit fingerprint, consistent across runs and platforms.
pub trait Fingerprint {
    fn fingerprint(&self) -> u64;
}

impl<T: AsRef<[u8]> + ?Sized> Fingerprint for T {
    fn fingerprint(&self) -> u64 {
        stable_hash_bytes(self.as_ref())
    }
}

/// Lower 32 bits of a [`stable_hash_bytes`] digest paired with a prefix.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentTag {
    prefix: String,
    digest: u32,
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

    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    #[must_use]
    pub fn digest(&self) -> u32 {
        self.digest
    }

    #[must_use]
    pub fn digest_matches(&self, other: &Self) -> bool {
        self.digest == other.digest
    }
}

impl fmt::Display for ContentTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:08x}", self.prefix, self.digest)
    }
}

#[must_use]
pub fn content_tag(prefix: &str, payload: &[u8]) -> String {
    ContentTag::new(prefix, payload).to_string()
}
