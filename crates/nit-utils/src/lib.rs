//! Shared utilities: atomic I/O, hashing, paths, and timestamps.
//!
//! The root module re-exports common items and provides cross-cutting
//! helpers — [`Fingerprint`] for content hashing, [`ContentTag`] for
//! digest-prefixed labels, and [`ensure_dir`] for recursive mkdir.

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

// ---------------------------------------------------------------------------
// Fingerprint — stable 64-bit content hashing trait
// ---------------------------------------------------------------------------

/// Trait for types that produce a stable 64-bit content fingerprint.
///
/// Every implementation delegates to [`stable_hash_bytes`], keeping the
/// digest algorithm consistent workspace-wide. Results are deterministic
/// across runs and platforms.
pub trait Fingerprint {
    /// Returns a deterministic 64-bit hash for deduplication and
    /// content-addressing.
    fn fingerprint(&self) -> u64;
}

impl Fingerprint for [u8] {
    #[inline]
    fn fingerprint(&self) -> u64 {
        stable_hash_bytes(self)
    }
}

impl Fingerprint for str {
    #[inline]
    fn fingerprint(&self) -> u64 {
        stable_hash_bytes(self.as_bytes())
    }
}

/// Delegates through `Deref` to the `[u8]` implementation.
impl Fingerprint for Vec<u8> {
    #[inline]
    fn fingerprint(&self) -> u64 {
        self.as_slice().fingerprint()
    }
}

/// Delegates through `Deref` to the `str` implementation.
impl Fingerprint for String {
    #[inline]
    fn fingerprint(&self) -> u64 {
        self.as_str().fingerprint()
    }
}

// ---------------------------------------------------------------------------
// ContentTag — prefix + truncated BLAKE3 digest
// ---------------------------------------------------------------------------

/// A content-derived tag pairing a human-readable prefix with a hex digest.
///
/// Built from the lower 32 bits of a [`stable_hash_bytes`] digest. The
/// display format is `{prefix}-{hex8}`, e.g. `"v2-a1b2c3d4"`. Use this
/// struct when you need to inspect the prefix or digest independently;
/// use the free function [`content_tag`] for quick `String` production.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentTag {
    prefix: String,
    digest: u32,
}

impl ContentTag {
    /// Hashes `payload` with BLAKE3, truncates to 32 bits, and pairs the
    /// result with `prefix`.
    #[must_use]
    pub fn new(prefix: &str, payload: &[u8]) -> Self {
        let raw = stable_hash_bytes(payload);
        let digest = (raw & 0xFFFF_FFFF) as u32;
        Self {
            prefix: prefix.to_owned(),
            digest,
        }
    }

    /// Returns the human-readable prefix portion.
    #[inline]
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Returns the raw 32-bit truncated digest.
    #[inline]
    #[must_use]
    pub fn digest(&self) -> u32 {
        self.digest
    }

    /// Returns `true` when two tags share the same digest regardless of
    /// their prefix. Useful for detecting content matches across namespaces.
    #[inline]
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

// ---------------------------------------------------------------------------
// Free functions — directory helpers and tag shorthand
// ---------------------------------------------------------------------------

/// Produces a `{prefix}-{hex8}` content tag string from a BLAKE3 hash of
/// `payload`.
///
/// Equivalent to `ContentTag::new(prefix, payload).to_string()`. Prefer
/// [`ContentTag`] directly when you need structured access to the components.
#[inline]
#[must_use]
pub fn content_tag(prefix: &str, payload: &[u8]) -> String {
    ContentTag::new(prefix, payload).to_string()
}

/// Creates `target` and all missing parent directories, returning the path.
///
/// A thin wrapper around [`std::fs::create_dir_all`] that returns the
/// input on success, enabling fluent chaining.
///
/// # Errors
///
/// Propagates [`io::Error`] on permission or filesystem failures.
#[inline]
pub fn ensure_dir(target: &Path) -> io::Result<&Path> {
    create_dir_all(target)?;
    Ok(target)
}
