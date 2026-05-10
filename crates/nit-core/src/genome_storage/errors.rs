//! Internal failure categorization for the file-per-report cache.
//!
//! Errors are not surfaced through the public API — persists are best-effort
//! and load failures silently skip the offending file. Keeping the typed
//! variants in a dedicated module lets the persist path use `?`/`From` for
//! readability and gives future telemetry hooks a stable surface to attach.

use std::fmt;
use std::io;

#[derive(Debug)]
pub(super) enum CacheError {
    Io(io::Error),
    MissingParent,
}

impl From<io::Error> for CacheError {
    fn from(err: io::Error) -> Self {
        CacheError::Io(err)
    }
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheError::Io(err) => write!(f, "genome cache io: {err}"),
            CacheError::MissingParent => f.write_str("genome cache: report path has no parent"),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let CacheError::Io(err) = self {
            Some(err)
        } else {
            None
        }
    }
}
