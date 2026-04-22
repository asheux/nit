//! Rate-limited debug/error logging for the syntax worker. Keeps the
//! stderr stream quiet under sustained highlight traffic.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tracing::{debug, error};

use crate::highlight::HighlightSnapshot;

/// Single-slot throttle: `f` runs at most once per `interval`. Seeded so the
/// first call always fires, even before `interval` has elapsed at process
/// start.
pub(super) struct RateLimiter {
    state: OnceLock<Mutex<Instant>>,
    interval: Duration,
}

impl RateLimiter {
    pub(super) const fn new(interval: Duration) -> Self {
        Self {
            state: OnceLock::new(),
            interval,
        }
    }

    pub(super) fn throttled(&self, f: impl FnOnce()) {
        let now = Instant::now();
        let guard = self.state.get_or_init(|| Mutex::new(now - self.interval));
        let mut last = guard.lock().unwrap();
        if now.duration_since(*last) >= self.interval {
            *last = now;
            f();
        }
    }
}

static LOG_COMPLETE: RateLimiter = RateLimiter::new(Duration::from_secs(1));
static LOG_ERROR: RateLimiter = RateLimiter::new(Duration::from_secs(1));

pub(super) fn log_completion(buffer_id: usize, version: u64, snapshot: &HighlightSnapshot) {
    LOG_COMPLETE.throttled(|| {
        let span_count: usize = snapshot.per_line.iter().map(|l| l.len()).sum();
        debug!(
            buffer_id,
            version,
            span_count,
            duration_ms = snapshot.duration_ms,
            "syntax highlight complete"
        );
    });
}

pub(super) fn log_error(buffer_id: usize, version: u64, err: &anyhow::Error) {
    LOG_ERROR.throttled(|| {
        error!(buffer_id, version, error = %err, "syntax highlight error");
    });
}
