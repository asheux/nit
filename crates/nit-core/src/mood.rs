//! Mood — system-wide global modulator over the living-system substrate.
//!
//! Three states (Exploration / Consolidation / Defensive) bias how
//! aggressive or tolerant the primitives are. Mood changes auto-transition
//! based on recent substrate pressure (ClaimViolation + Warning +
//! HelpNeeded density in the last N generations) with hysteresis, and
//! can be manually set via AgentBusEvent::SetMood.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mood {
    Exploration,
    #[default]
    Consolidation,
    Defensive,
}

/// Per-mood knob table read by the relevant primitives.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MoodModulation {
    pub metabolic_tick: Duration,
    pub arbiter_max_per_tick: usize,
    pub repeat_failure_threshold: usize,
    /// Multiplier applied to `SignalKind::decay_rate()` when computing
    /// effective strength. >1 fades signals faster (more forgetful),
    /// <1 slows decay (preserves history). 1.0 = default (no change).
    pub signal_decay_multiplier: f32,
    /// Multiplier applied to the auto-claim TTL in the `FileWrite` arm.
    /// Values above 1 hold claims longer (defensive); below 1 cycles them
    /// faster (exploration). 1.0 = default.
    pub claim_ttl_multiplier: f32,
}

impl Mood {
    pub const fn modulation(self) -> MoodModulation {
        match self {
            Mood::Exploration => MoodModulation {
                metabolic_tick: Duration::from_secs(10),
                arbiter_max_per_tick: 1,
                repeat_failure_threshold: 3,
                // Signals fade faster — forget and retry sooner.
                signal_decay_multiplier: 1.1,
                // Shorter TTLs — more turnover, more freedom to overwrite.
                claim_ttl_multiplier: 0.75,
            },
            Mood::Consolidation => MoodModulation {
                metabolic_tick: Duration::from_secs(5),
                arbiter_max_per_tick: 2,
                repeat_failure_threshold: 2,
                signal_decay_multiplier: 1.0,
                claim_ttl_multiplier: 1.0,
            },
            Mood::Defensive => MoodModulation {
                metabolic_tick: Duration::from_secs(3),
                arbiter_max_per_tick: 4,
                repeat_failure_threshold: 1,
                // Slower decay — preserve warnings longer.
                signal_decay_multiplier: 0.85,
                // Longer TTLs — hold resources tighter.
                claim_ttl_multiplier: 1.5,
            },
        }
    }
}

/// Auto-transition decision. Returns `Some(new_mood)` to apply, else `None`.
/// `pressure` is recent ClaimViolation+Warning+HelpNeeded count;
/// `quiet_streak` gates re-entering Exploration so a single low-pressure
/// tick doesn't flip back from Consolidation.
pub fn auto_transition(current: Mood, pressure: usize, quiet_streak: u32) -> Option<Mood> {
    match current {
        Mood::Consolidation => {
            if pressure >= 8 {
                Some(Mood::Defensive)
            } else if pressure <= 1 && quiet_streak >= 3 {
                Some(Mood::Exploration)
            } else {
                None
            }
        }
        Mood::Defensive => {
            if pressure <= 4 {
                Some(Mood::Consolidation)
            } else {
                None
            }
        }
        Mood::Exploration => {
            if pressure >= 3 {
                Some(Mood::Consolidation)
            } else {
                None
            }
        }
    }
}

pub const MOOD_PRESSURE_WINDOW_GENS: u64 = 10;
pub const MOOD_OVERRIDE_LOCK_GENS: u64 = 20;
pub const MOOD_QUIET_PRESSURE_MAX: usize = 1;

#[cfg(test)]
#[path = "tests/mood.rs"]
mod tests;
