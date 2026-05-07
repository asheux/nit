//! Observer framework — the first consumer of substrate signals.
//!
//! Invariant: emissions are buffered across all observers within one tick
//! before being written back. Observer N never sees observer M's emission
//! within the same tick (intra-tick cascade safeguard).

use std::collections::HashMap;
use std::hash::Hash;

use crate::state::AppState;
use crate::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};

pub mod global_heat;
pub mod repeat_failure;
pub mod sparse_plan;

/// Initial strength for observer-emitted signals — higher than the worker
/// default (1.0) so structural facts outlast worker-emitted transients.
pub const OBSERVER_INITIAL_STRENGTH: f32 = 1.5;

#[derive(Clone, Debug)]
pub struct ObservedEmission {
    pub kind: SignalKind,
    pub target: SignalTarget,
    pub initial_strength: f32,
    pub payload: serde_json::Value,
}

impl ObservedEmission {
    /// Construct an emission with the standard observer initial strength.
    pub fn new(kind: SignalKind, target: SignalTarget, payload: serde_json::Value) -> Self {
        Self {
            kind,
            target,
            initial_strength: OBSERVER_INITIAL_STRENGTH,
            payload,
        }
    }
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

pub(crate) fn signals_in_window<'a>(
    sub: &'a SubstrateState,
    window_start: u64,
) -> impl Iterator<Item = &'a Signal> + 'a {
    sub.signals
        .values()
        .filter(move |s| s.posted_at_gen >= window_start)
}

pub(crate) fn recent_global_warning(
    sub: &SubstrateState,
    observer_name: &str,
    window_start: u64,
) -> bool {
    signals_in_window(sub, window_start).any(|s| {
        s.kind == SignalKind::Warning
            && s.posted_by == observer_name
            && matches!(s.target, SignalTarget::Global)
    })
}

pub(crate) fn iter_recent_warnings<'a>(
    sub: &'a SubstrateState,
    window_start: u64,
) -> impl Iterator<Item = &'a Signal> + 'a {
    signals_in_window(sub, window_start).filter(|s| s.kind == SignalKind::Warning)
}

pub(crate) fn recent_help_targets(
    sub: &SubstrateState,
    observer_name: &str,
    window_start: u64,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for s in signals_in_window(sub, window_start) {
        if s.kind != SignalKind::HelpNeeded || s.posted_by != observer_name {
            continue;
        }
        if let SignalTarget::Agent { agent_id } = &s.target {
            out.insert(agent_id.clone());
        }
    }
    out
}

pub(crate) fn count_by_key<'a, K, F>(
    signals: impl Iterator<Item = &'a Signal>,
    key: F,
) -> HashMap<K, usize>
where
    K: Eq + Hash,
    F: Fn(&'a Signal) -> K,
{
    let mut out: HashMap<K, usize> = HashMap::new();
    for s in signals {
        *out.entry(key(s)).or_insert(0) += 1;
    }
    out
}

pub(crate) fn count_with_unique<'a, K, F, G>(
    signals: impl Iterator<Item = &'a Signal>,
    key: F,
    value: G,
) -> HashMap<K, (usize, Vec<String>)>
where
    K: Eq + Hash,
    F: Fn(&'a Signal) -> Option<K>,
    G: Fn(&'a Signal) -> Option<String>,
{
    let mut out: HashMap<K, (usize, Vec<String>)> = HashMap::new();
    for s in signals {
        let Some(k) = key(s) else { continue };
        let entry = out.entry(k).or_insert((0, Vec::new()));
        entry.0 += 1;
        if let Some(v) = value(s) {
            if !v.is_empty() && !entry.1.contains(&v) {
                entry.1.push(v);
            }
        }
    }
    out
}

pub(crate) fn payload_string(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload.get(key).and_then(|v| v.as_str()).map(String::from)
}

#[cfg(test)]
#[path = "../tests/observers.rs"]
mod tests;
