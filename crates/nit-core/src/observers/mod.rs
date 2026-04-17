//! Observer framework — the first consumer of substrate signals.
//!
//! Observers are stateless fn-pointers that read `&AppState` and return
//! emissions. The registry attributes `posted_by = "observer:{name}"` and
//! mints ids via the substrate, so an observer cannot self-spoof as an agent.
//!
//! Invariant: emissions are buffered across all observers within one tick
//! before being written back. Observer N never sees observer M's emission
//! within the same tick (intra-tick cascade safeguard).

use crate::state::AppState;
use crate::substrate::{SignalKind, SignalTarget};

pub mod global_heat;
pub mod repeat_failure;

/// Initial strength for observer-emitted signals — higher than the worker
/// default (1.0) so structural facts outlast worker-emitted transients.
pub const OBSERVER_INITIAL_STRENGTH: f32 = 1.5;

/// An emission produced by an observer — the registry attaches id and
/// posted_by before inserting into the substrate.
#[derive(Clone, Debug)]
pub struct ObservedEmission {
    pub kind: SignalKind,
    pub target: SignalTarget,
    pub initial_strength: f32,
    pub payload: serde_json::Value,
}

pub type ObserverFn = fn(&AppState) -> Vec<ObservedEmission>;

pub struct Observer {
    pub name: &'static str,
    pub run: ObserverFn,
}

pub const REGISTERED_OBSERVERS: &[Observer] = &[
    repeat_failure::OBSERVER,
    global_heat::OBSERVER,
];

/// Runs every registered observer and collects their emissions.
/// Observer ordering within this function is the REGISTERED_OBSERVERS
/// array order. No observer sees another's emissions within this call —
/// all observers read the same `AppState` snapshot.
pub fn run_all(state: &AppState) -> Vec<(&'static str, ObservedEmission)> {
    let mut out = Vec::new();
    for obs in REGISTERED_OBSERVERS {
        for em in (obs.run)(state) {
            out.push((obs.name, em));
        }
    }
    out
}

#[cfg(test)]
#[path = "../tests/observers.rs"]
mod tests;
