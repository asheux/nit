//! GlobalHeatObserver — if total signal count exceeds threshold AND no
//! recent observer:global_heat Warning already exists, emit a Global Warning.

use super::{ObservedEmission, Observer, OBSERVER_INITIAL_STRENGTH};
use crate::state::AppState;
use crate::substrate::{SignalKind, SignalTarget};

pub const OBSERVER: Observer = Observer {
    name: "global_heat",
    run: observe,
};

const THRESHOLD: usize = 100;
const COOLDOWN_GENS: u64 = 10;

fn observe(state: &AppState) -> Vec<ObservedEmission> {
    let sub = &state.substrate;
    let n = sub.signals.len();
    if n <= THRESHOLD {
        return Vec::new();
    }
    let current_gen = sub.current_generation();
    let cooldown_start = current_gen.saturating_sub(COOLDOWN_GENS);

    // Cooldown: skip if we emitted a Global Warning within COOLDOWN_GENS.
    let recently_emitted = sub.signals.values().any(|s| {
        s.kind == SignalKind::Warning
            && s.posted_by == "observer:global_heat"
            && matches!(s.target, SignalTarget::Global)
            && s.posted_at_gen >= cooldown_start
    });
    if recently_emitted {
        return Vec::new();
    }

    vec![ObservedEmission {
        kind: SignalKind::Warning,
        target: SignalTarget::Global,
        initial_strength: OBSERVER_INITIAL_STRENGTH,
        payload: serde_json::json!({
            "reason": "global_heat",
            "signal_count": n,
            "threshold": THRESHOLD,
        }),
    }]
}
