//! Shared utilities: atomic I/O, hashing, paths, and timestamps.

#![forbid(unsafe_code)]

use std::fmt;
use std::fs::create_dir_all;
use std::io;
use std::path::Path;

pub mod fs;
pub mod hashing;
pub mod paths;
pub mod time;

pub use fs::write_atomic;
pub use hashing::{stable_hash_bytes, SplitMix64};

/// Crate version derived from the `Cargo.toml` manifest at compile time.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Trait for types that produce a stable 64-bit content fingerprint.
///
/// Delegates to [`stable_hash_bytes`], keeping the digest algorithm consistent
/// workspace-wide. Results are deterministic across runs and platforms.
pub trait Fingerprint {
    fn fingerprint(&self) -> u64;
}

impl Fingerprint for [u8] {
    fn fingerprint(&self) -> u64 {
        stable_hash_bytes(self)
    }
}

impl Fingerprint for str {
    fn fingerprint(&self) -> u64 {
        stable_hash_bytes(self.as_bytes())
    }
}

impl Fingerprint for Vec<u8> {
    fn fingerprint(&self) -> u64 {
        self.as_slice().fingerprint()
    }
}

impl Fingerprint for String {
    fn fingerprint(&self) -> u64 {
        self.as_str().fingerprint()
    }
}

/// A content-derived tag pairing a human-readable prefix with a hex digest.
///
/// Built from the lower 32 bits of a [`stable_hash_bytes`] digest. Display
/// format is `{prefix}-{hex8}`, e.g. `"v2-a1b2c3d4"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentTag {
    prefix: String,
    digest: u32,
}

impl ContentTag {
    /// Hashes `payload` with BLAKE3, truncates to 32 bits, and pairs with `prefix`.
    #[must_use]
    pub fn new(prefix: &str, payload: &[u8]) -> Self {
        let raw = stable_hash_bytes(payload);
        let digest = (raw & 0xFFFF_FFFF) as u32;
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

    /// Returns `true` when two tags share the same digest regardless of prefix.
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

/// Produces a `{prefix}-{hex8}` content tag string from `payload`.
///
/// Shorthand for `ContentTag::new(prefix, payload).to_string()`.
#[must_use]
pub fn content_tag(prefix: &str, payload: &[u8]) -> String {
    ContentTag::new(prefix, payload).to_string()
}

/// Creates `target` and all missing parent directories, returning the path.
///
/// # Errors
///
/// Propagates [`io::Error`] on permission or filesystem failures.
pub fn ensure_dir(target: &Path) -> io::Result<&Path> {
    create_dir_all(target)?;
    Ok(target)
}
