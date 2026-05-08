//! Mood — system-wide global modulator over the living-system substrate.
//!
//! Three states (Exploration / Consolidation / Defensive) bias how
//! aggressive or tolerant the primitives are. Mood changes auto-transition
//! based on recent substrate pressure (ClaimViolation + Warning +
//! HelpNeeded density in the last [`MOOD_PRESSURE_WINDOW_GENS`]
//! generations) with hysteresis, and can be manually set via
//! [`crate::agent_bus::AgentBusEvent::SetMood`].

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
///
/// The two multiplier fields (`signal_decay_multiplier`,
/// `claim_ttl_multiplier`) modulate substrate dynamics: > 1 fades signals
/// or shortens claim TTLs (faster turnover, more freedom to overwrite);
/// < 1 preserves them (defensive memory, tighter resource hold). 1.0 is
/// the no-op baseline.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MoodModulation {
    pub metabolic_tick: Duration,
    pub arbiter_max_per_tick: usize,
    pub repeat_failure_threshold: usize,
    pub signal_decay_multiplier: f32,
    pub claim_ttl_multiplier: f32,
}

impl Mood {
    pub const fn modulation(self) -> MoodModulation {
        match self {
            Mood::Exploration => MoodModulation {
                metabolic_tick: Duration::from_secs(10),
                arbiter_max_per_tick: 1,
                repeat_failure_threshold: 3,
                signal_decay_multiplier: 1.1,
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
                signal_decay_multiplier: 0.85,
                claim_ttl_multiplier: 1.5,
            },
        }
    }
}

// Auto-transition thresholds. Hysteresis is encoded by making the rise
// pressure (`PRESSURE_RAISE_*`) strictly above the drop pressure
// (`PRESSURE_DROP_*`) for each direction, so oscillating loads don't
// thrash the mood machine.
const PRESSURE_RAISE_TO_DEFENSIVE: usize = 8;
const PRESSURE_DROP_FROM_DEFENSIVE: usize = 4;
const PRESSURE_RAISE_TO_CONSOLIDATION: usize = 3;
const PRESSURE_DROP_TO_EXPLORATION: usize = MOOD_QUIET_PRESSURE_MAX;
const STREAK_TO_EXPLORATION: u32 = 3;

/// Auto-transition decision. Returns `Some(new_mood)` to apply, else
/// `None`. `pressure` is the recent ClaimViolation+Warning+HelpNeeded
/// count; `quiet_streak` gates re-entering Exploration so a single
/// low-pressure tick doesn't flip back from Consolidation.
pub fn auto_transition(current: Mood, pressure: usize, quiet_streak: u32) -> Option<Mood> {
    match current {
        Mood::Consolidation => {
            if pressure >= PRESSURE_RAISE_TO_DEFENSIVE {
                Some(Mood::Defensive)
            } else if pressure <= PRESSURE_DROP_TO_EXPLORATION
                && quiet_streak >= STREAK_TO_EXPLORATION
            {
                Some(Mood::Exploration)
            } else {
                None
            }
        }
        Mood::Defensive => {
            (pressure <= PRESSURE_DROP_FROM_DEFENSIVE).then_some(Mood::Consolidation)
        }
        Mood::Exploration => {
            (pressure >= PRESSURE_RAISE_TO_CONSOLIDATION).then_some(Mood::Consolidation)
        }
    }
}

pub const MOOD_PRESSURE_WINDOW_GENS: u64 = 10;
pub const MOOD_OVERRIDE_LOCK_GENS: u64 = 20;
pub const MOOD_QUIET_PRESSURE_MAX: usize = 1;

#[cfg(test)]
#[path = "tests/mood.rs"]
mod tests;
