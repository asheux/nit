//! Wall-clock timestamp helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Falls back to `0` if the system clock predates the epoch.
#[must_use]
pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
