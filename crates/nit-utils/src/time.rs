use std::time::{SystemTime, UNIX_EPOCH};

#[inline]
#[must_use]
pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}
