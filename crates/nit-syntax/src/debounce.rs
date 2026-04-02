//! Debounce timer for rate-limiting syntax rehighlight requests.
//!
//! The editor fires rehighlight events on every keystroke; [`Debouncer`]
//! collapses rapid events and only reports readiness once the user pauses
//! for a configurable quiet period.
//!
//! # Usage
//!
//! ```ignore
//! let mut debounce = Debouncer::new(50);
//! debounce.mark();     // keystroke happened
//! // ... later, in the event loop ...
//! if debounce.ready() {
//!     debounce.clear();
//!     // trigger rehighlight
//! }
//! ```

use std::fmt;
use std::time::{Duration, Instant};

// ── Constants ──────────────────────────────────────────────────────────────

/// Default quiet period in milliseconds used by [`Debouncer::default`].
///
/// Matches the `debounce_ms` value in [`crate::SyntaxConfig::default`],
/// ensuring consistent behaviour when both are created with defaults.
const DEFAULT_QUIET_PERIOD_MS: u64 = 50;

// ── Phase enum ─────────────────────────────────────────────────────────────

/// Discrete state of a [`Debouncer`] at a point in time.
///
/// Returned by [`Debouncer::phase`] for pattern-matching in event loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebouncerPhase {
    /// No event recorded — the debouncer is at rest.
    Idle,
    /// An event was recorded but the quiet period has not yet elapsed.
    Pending,
    /// The quiet period has elapsed — ready to fire.
    Ready,
}

impl fmt::Display for DebouncerPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Ready => "ready",
        };
        f.write_str(label)
    }
}

// ── Core type ──────────────────────────────────────────────────────────────

/// Rate-limiter that reports readiness once a configurable delay has
/// elapsed since the last recorded event.
///
/// The debouncer transitions through three logical states:
///
/// | State       | `ready()` | `pending()` | Transition            |
/// |-------------|-----------|-------------|-----------------------|
/// | **Idle**    | `false`   | `false`     | → Pending via `mark`  |
/// | **Pending** | `false`   | `true`      | → Ready after delay   |
/// | **Ready**   | `true`    | `false`     | → Idle via `clear`    |
#[derive(Debug, Clone)]
pub struct Debouncer {
    /// Minimum quiet period before the debouncer reports readiness.
    quiet_period: Duration,
    /// Timestamp of the most recent event, or `None` when idle.
    last_event_at: Option<Instant>,
}

// ── Default implementation ─────────────────────────────────────────────────

impl Default for Debouncer {
    /// Creates a debouncer with a [`DEFAULT_QUIET_PERIOD_MS`] delay.
    fn default() -> Self {
        Self::new(DEFAULT_QUIET_PERIOD_MS)
    }
}

// ── Construction ───────────────────────────────────────────────────────────

impl Debouncer {
    /// Create a debouncer with a minimum quiet period of `delay_ms`
    /// milliseconds between events.
    pub fn new(delay_ms: u64) -> Self {
        Self {
            quiet_period: Duration::from_millis(delay_ms),
            last_event_at: None,
        }
    }
}

// ── Event recording ────────────────────────────────────────────────────────

impl Debouncer {
    /// Record that an event just occurred, resetting the delay window.
    ///
    /// Calling `mark` moves the debouncer from idle or ready back to the
    /// pending state, restarting the quiet period from this instant.
    pub fn mark(&mut self) {
        self.last_event_at = Some(Instant::now());
    }

    /// Reset the debouncer to the idle state, discarding any pending
    /// event timestamp. A subsequent call to [`ready`](Self::ready)
    /// will return `false` until a new [`mark`](Self::mark) fires.
    pub fn clear(&mut self) {
        self.last_event_at = None;
    }
}

// ── Phase queries ──────────────────────────────────────────────────────────

impl Debouncer {
    /// Returns the current [`DebouncerPhase`] for pattern matching.
    pub fn phase(&self) -> DebouncerPhase {
        match self.last_event_at {
            None => DebouncerPhase::Idle,
            Some(ts) if ts.elapsed() >= self.quiet_period => DebouncerPhase::Ready,
            Some(_) => DebouncerPhase::Pending,
        }
    }

    /// Returns `true` when the full quiet period has elapsed since the
    /// last [`mark`](Self::mark).
    ///
    /// Returns `false` when idle (no event recorded) or still within
    /// the quiet period.
    pub fn ready(&self) -> bool {
        self.last_event_at
            .is_some_and(|ts| ts.elapsed() >= self.quiet_period)
    }

    /// Returns `true` when an event has been recorded but the quiet
    /// period has not yet elapsed.
    pub fn pending(&self) -> bool {
        self.last_event_at
            .is_some_and(|ts| ts.elapsed() < self.quiet_period)
    }

    /// The configured minimum quiet period between events.
    pub fn delay(&self) -> Duration {
        self.quiet_period
    }
}
