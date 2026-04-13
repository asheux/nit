//! Debounce timer for rate-limiting syntax rehighlight requests.

use std::fmt;
use std::time::{Duration, Instant};

const DEFAULT_QUIET_PERIOD_MS: u64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebouncerPhase {
    Idle,
    Pending,
    Ready,
}

impl fmt::Display for DebouncerPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Ready => "ready",
        })
    }
}

/// Rate-limiter that collapses rapid events and reports readiness
/// once a configurable quiet period elapses.
///
/// | State       | `ready()` | `pending()` | Transition            |
/// |-------------|-----------|-------------|-----------------------|
/// | **Idle**    | `false`   | `false`     | → Pending via `mark`  |
/// | **Pending** | `false`   | `true`      | → Ready after delay   |
/// | **Ready**   | `true`    | `false`     | → Idle via `clear`    |
#[derive(Debug, Clone)]
pub struct Debouncer {
    quiet_period: Duration,
    last_event_at: Option<Instant>,
}

impl Default for Debouncer {
    fn default() -> Self {
        Self::new(DEFAULT_QUIET_PERIOD_MS)
    }
}

impl Debouncer {
    pub fn new(delay_ms: u64) -> Self {
        Self {
            quiet_period: Duration::from_millis(delay_ms),
            last_event_at: None,
        }
    }

    pub fn mark(&mut self) {
        self.last_event_at = Some(Instant::now());
    }

    pub fn clear(&mut self) {
        self.last_event_at = None;
    }

    pub fn phase(&self) -> DebouncerPhase {
        match self.last_event_at {
            None => DebouncerPhase::Idle,
            Some(ts) if ts.elapsed() >= self.quiet_period => DebouncerPhase::Ready,
            Some(_) => DebouncerPhase::Pending,
        }
    }

    /// Returns `false` when idle (no event recorded).
    pub fn ready(&self) -> bool {
        self.last_event_at
            .is_some_and(|ts| ts.elapsed() >= self.quiet_period)
    }

    /// Returns `false` when idle or already ready.
    pub fn pending(&self) -> bool {
        self.last_event_at
            .is_some_and(|ts| ts.elapsed() < self.quiet_period)
    }

    pub fn delay(&self) -> Duration {
        self.quiet_period
    }
}
