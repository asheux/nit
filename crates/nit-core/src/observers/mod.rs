//! Observer framework — the first consumer of substrate signals.
//!
//! Invariant: emissions are buffered across all observers within one tick
//! before being written back. Observer N never sees observer M's emission
//! within the same tick (intra-tick cascade safeguard).

use std::collections::HashSet;

use crate::state::AppState;
use crate::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};

pub mod global_heat;
pub mod repeat_failure;
pub mod sparse_plan;

/// Initial strength for observer-emitted signals — higher than the worker
/// default (1.0) so structural facts outlast worker-emitted transients.
pub const OBSERVER_INITIAL_STRENGTH: f32 = 1.5;

/// Emission produced by an observer; the registry attaches id and posted_by
/// before inserting into the substrate.
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
    sparse_plan::OBSERVER,
];

/// All observers read the same `AppState` snapshot — caller is expected to
/// commit emissions only after this returns.
pub fn run_all(state: &AppState) -> Vec<(&'static str, ObservedEmission)> {
    let mut out = Vec::new();
    for obs in REGISTERED_OBSERVERS {
        for em in (obs.run)(state) {
            out.push((obs.name, em));
        }
    }
    out
}

pub(super) fn recent_help_targets(
    sub: &SubstrateState,
    observer_name: &str,
    window_start: u64,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for s in sub.signals.values() {
        if s.kind != SignalKind::HelpNeeded
            || s.posted_at_gen < window_start
            || s.posted_by != observer_name
        {
            continue;
        }
        if let SignalTarget::Agent { agent_id } = &s.target {
            out.insert(agent_id.clone());
        }
    }
    out
}

pub(super) fn recent_global_warning(
    sub: &SubstrateState,
    observer_name: &str,
    window_start: u64,
) -> bool {
    sub.signals.values().any(|s| {
        s.kind == SignalKind::Warning
            && s.posted_by == observer_name
            && matches!(s.target, SignalTarget::Global)
            && s.posted_at_gen >= window_start
    })
}

pub(super) fn iter_recent_warnings<'a>(
    sub: &'a SubstrateState,
    window_start: u64,
) -> impl Iterator<Item = &'a Signal> + 'a {
    sub.signals
        .values()
        .filter(move |s| s.kind == SignalKind::Warning && s.posted_at_gen >= window_start)
}

#[cfg(test)]
#[path = "../tests/observers.rs"]
mod tests;
