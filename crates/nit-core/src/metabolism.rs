//! Phase-7 metabolism — a wall-clock substrate sweep that runs
//! independently of turn boundaries.
//!
//! Invariant: `tick` does NOT advance `SubstrateState::generation`.
//! Generation tracks turns; metabolism tracks time.
//!
//! Today the only caller is `crates/nit-tui/src/app/mod.rs`'s
//! main-loop frame-time check; the interval is
//! [`METABOLIC_TICK_INTERVAL`].

use std::time::Duration;

use crate::state::AppState;
use crate::substrate::{Signal, SubstrateState};

pub const METABOLIC_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Mood-adjusted tick interval. Callers in the TUI frame-time check read
/// this instead of the bare `METABOLIC_TICK_INTERVAL` const so the wall
/// clock breathes with the system's mood.
pub fn tick_interval_for(mood: crate::mood::Mood) -> Duration {
    mood.modulation().metabolic_tick
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetabolicTickOutcome {
    pub claims_expired: usize,
    pub signals_pruned: usize,
    pub assumptions_expired: usize,
    pub observer_emissions: usize,
    pub arbiter_interventions: usize,
    pub mood_transitioned: bool,
    pub saved: bool,
}

impl MetabolicTickOutcome {
    pub fn is_noop(&self) -> bool {
        self.claims_expired == 0
            && self.signals_pruned == 0
            && self.assumptions_expired == 0
            && self.observer_emissions == 0
            && self.arbiter_interventions == 0
            && !self.mood_transitioned
            && !self.saved
    }
}

/// Wall-clock sweep: expire claims, prune signals, run observers,
/// run arbiters, conditionally save.  Does NOT advance the generation
/// counter.
pub fn tick(state: &mut AppState) -> MetabolicTickOutcome {
    let current_gen = state.substrate.current_generation();
    let claims_expired = state.substrate.expire_claims(current_gen);
    let assumptions_expired = state.substrate.expire_assumptions(current_gen);
    let signals_pruned = state
        .substrate
        .prune_signals_below(SubstrateState::DEFAULT_PRUNE_THRESHOLD);

    // Phase 9: evaluate mood auto-transition (once per metabolic tick).
    let override_active = state.substrate.mood_override_until_gen > current_gen;
    let pressure = state
        .substrate
        .pressure_in_window(crate::mood::MOOD_PRESSURE_WINDOW_GENS);

    // Quiet-streak tracking — ticks with low pressure increment, otherwise reset.
    if pressure <= crate::mood::MOOD_QUIET_PRESSURE_MAX {
        state.substrate.mood_quiet_streak = state.substrate.mood_quiet_streak.saturating_add(1);
    } else {
        state.substrate.mood_quiet_streak = 0;
    }

    let mood_transitioned = if override_active {
        false
    } else if let Some(new_mood) = crate::mood::auto_transition(
        state.substrate.mood,
        pressure,
        state.substrate.mood_quiet_streak,
    ) {
        let from = state.substrate.mood;
        state.substrate.mood = new_mood;
        let payload = serde_json::json!({
            "reason": "mood_auto_transition",
            "from": format!("{from:?}").to_lowercase(),
            "to": format!("{new_mood:?}").to_lowercase(),
            "pressure": pressure,
            "source": "auto",
        });
        let posted_by = "mood".to_string();
        let id = state.substrate.next_signal_id(&posted_by);
        let posted_at_gen = state.substrate.current_generation();
        state.substrate.emit_signal(crate::substrate::Signal {
            id,
            kind: crate::substrate::SignalKind::Warning,
            posted_by,
            posted_at_gen,
            target: crate::substrate::SignalTarget::Global,
            initial_strength: crate::substrate::SubstrateState::DEFAULT_INITIAL_STRENGTH,
            payload,
        });
        true
    } else {
        false
    };

    // Observer pass — mirrors the emission loop in
    // `AgentBusEvent::TurnCompleted::apply` around agent_bus.rs:577-591.
    let emissions = crate::observers::run_all(state);
    let observer_emissions = emissions.len();
    for (observer_name, em) in emissions {
        let posted_by = format!("observer:{observer_name}");
        let id = state.substrate.next_signal_id(&posted_by);
        let posted_at_gen = state.substrate.current_generation();
        state.substrate.emit_signal(Signal {
            id,
            kind: em.kind,
            posted_by,
            posted_at_gen,
            target: em.target,
            initial_strength: em.initial_strength,
            payload: em.payload,
        });
    }

    // Phase 6: arbiters run AFTER observers. Same three-line block as
    // `AgentBusEvent::TurnCompleted::apply`. `reduce_proposals` downgrades
    // to `EmitSignalOnly` if the retry cap is reached.
    let raw = crate::arbiters::run_all(state);
    let reduced =
        crate::arbiters::reduce_proposals(state, raw, crate::arbiters::ARBITER_RETRY_LIMIT);
    let arbiter_interventions = reduced.len();
    crate::arbiters::apply_interventions(state, reduced);

    let dirty = claims_expired > 0
        || signals_pruned > 0
        || assumptions_expired > 0
        || observer_emissions > 0
        || arbiter_interventions > 0
        || mood_transitioned;
    let saved = if dirty {
        state.substrate.save(&state.workspace_root).is_ok()
    } else {
        false
    };

    MetabolicTickOutcome {
        claims_expired,
        signals_pruned,
        assumptions_expired,
        observer_emissions,
        arbiter_interventions,
        mood_transitioned,
        saved,
    }
}

#[cfg(test)]
#[path = "tests/metabolism.rs"]
mod tests;
