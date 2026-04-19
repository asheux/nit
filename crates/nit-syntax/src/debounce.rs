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

/// Collapses bursts of events, firing once no mark has arrived for
/// `quiet_period`. Transitions:
/// `Idle → mark → Pending → elapsed → Ready → clear → Idle`.
#[derive(Debug, Clone)]
pub struct Debouncer {
    quiet_period: Duration,
    last_mark: Option<Instant>,
}

impl Default for Debouncer {
    fn default() -> Self {
        Self::new(DEFAULT_QUIET_PERIOD_MS)
    }
}

impl Debouncer {
    #[must_use]
    pub const fn new(quiet_period_ms: u64) -> Self {
        Self {
            quiet_period: Duration::from_millis(quiet_period_ms),
            last_mark: None,
        }
    }

    pub fn mark(&mut self) {
        self.last_mark = Some(Instant::now());
    }

    pub fn clear(&mut self) {
        self.last_mark = None;
    }

    #[must_use]
    pub fn phase(&self) -> DebouncerPhase {
        match self.last_mark {
            None => DebouncerPhase::Idle,
            Some(ts) if ts.elapsed() >= self.quiet_period => DebouncerPhase::Ready,
            Some(_) => DebouncerPhase::Pending,
        }
    }

    #[must_use]
    pub fn ready(&self) -> bool {
        self.phase() == DebouncerPhase::Ready
    }

    #[must_use]
    pub fn pending(&self) -> bool {
        self.phase() == DebouncerPhase::Pending
    }

    #[must_use]
    pub const fn delay(&self) -> Duration {
        self.quiet_period
    }
}
