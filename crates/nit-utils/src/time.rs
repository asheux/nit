//! Wall-clock timestamp helpers.
//!
//! Provides convenience functions for obtaining the current system time as
//! integer timestamps, useful for log entries, snapshot metadata, and event
//! ordering.

use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current wall-clock time as milliseconds since the Unix epoch.
///
/// Falls back to `0` if the system clock is set before the epoch (which is
/// effectively impossible on modern hardware, but avoids a panic).
///
/// # Note
///
/// This function is part of the public API surface. External consumers or
/// downstream tooling may depend on it.
#[inline]
#[must_use]
pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
