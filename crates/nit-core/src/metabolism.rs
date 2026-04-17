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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetabolicTickOutcome {
    pub claims_expired: usize,
    pub signals_pruned: usize,
    pub assumptions_expired: usize,
    pub observer_emissions: usize,
    pub saved: bool,
}

impl MetabolicTickOutcome {
    pub fn is_noop(&self) -> bool {
        self.claims_expired == 0
            && self.signals_pruned == 0
            && self.assumptions_expired == 0
            && self.observer_emissions == 0
            && !self.saved
    }
}

/// Wall-clock sweep: expire claims, prune signals, run observers,
/// conditionally save.  Does NOT advance the generation counter.
pub fn tick(state: &mut AppState) -> MetabolicTickOutcome {
    let current_gen = state.substrate.current_generation();
    let claims_expired = state.substrate.expire_claims(current_gen);
    let assumptions_expired = state.substrate.expire_assumptions(current_gen);
    let signals_pruned = state
        .substrate
        .prune_signals_below(SubstrateState::DEFAULT_PRUNE_THRESHOLD);

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

    let dirty = claims_expired > 0
        || signals_pruned > 0
        || assumptions_expired > 0
        || observer_emissions > 0;
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
        saved,
    }
}

#[cfg(test)]
#[path = "tests/metabolism.rs"]
mod tests;
