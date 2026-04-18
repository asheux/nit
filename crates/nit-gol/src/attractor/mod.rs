//! Attractor detection for Game of Life simulations.
//!
//! Detects fixed points and periodic cycles during grid evolution by
//! maintaining a fingerprint history of observed states. Supports both
//! simple and protocol-aware (multi-phase) observation.

mod detector;
mod fingerprint;

pub use detector::{AttractorConfig, AttractorDetector};
pub use fingerprint::Fingerprint;

/// Additional context for protocol-aware attractor detection.
///
/// When a simulation runs a multi-phase protocol, these fields are
/// mixed into the grid fingerprint so that identical grids in
/// different protocol states are not mistaken for a cycle.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttractorExtra {
    /// Stable digest of the protocol definition.
    pub protocol_hash: u64,
    /// Zero-based index of the active phase within the protocol.
    pub phase_idx: u32,
    /// Generations elapsed since this phase began; reset to zero when a new phase starts.
    pub step_in_phase: u32,
}

/// Events emitted when an attractor is detected.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AttractorEvent {
    /// The grid is identical to its successor (period-1 attractor).
    FixedPoint { gen: u64 },
    /// A previously seen state was observed again.
    Cycle {
        gen: u64,
        first_seen: u64,
        period: u64,
        transient: u64,
    },
}

/// Policy controlling whether the simulation auto-stops on attractors.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AutoStopPolicy {
    /// Never auto-stop.
    Off,
    /// Stop only on fixed points.
    Fixed,
    /// Stop on any repeat (fixed point or cycle).
    Repeat,
}

impl AutoStopPolicy {
    /// Returns `true` if the given event should halt the simulation.
    pub fn should_stop(self, event: &AttractorEvent) -> bool {
        match self {
            AutoStopPolicy::Off => false,
            AutoStopPolicy::Fixed => matches!(event, AttractorEvent::FixedPoint { .. }),
            AutoStopPolicy::Repeat => true,
        }
    }

    /// Cycle to the next policy variant in round-robin order.
    pub fn next(self) -> Self {
        match self {
            AutoStopPolicy::Off => AutoStopPolicy::Fixed,
            AutoStopPolicy::Fixed => AutoStopPolicy::Repeat,
            AutoStopPolicy::Repeat => AutoStopPolicy::Off,
        }
    }

    /// Human-readable label for display.
    pub fn label(self) -> &'static str {
        match self {
            AutoStopPolicy::Off => "Off",
            AutoStopPolicy::Fixed => "Fixed",
            AutoStopPolicy::Repeat => "Repeat",
        }
    }
}

impl std::fmt::Display for AutoStopPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
#[path = "../test_modules/attractor.rs"]
mod tests;
