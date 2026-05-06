//! Global Warning when active-signal count exceeds THRESHOLD, with a
//! COOLDOWN_GENS-generation suppression window.

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
    let cooldown_start = sub.current_generation().saturating_sub(COOLDOWN_GENS);
    if super::recent_global_warning(sub, "observer:global_heat", cooldown_start) {
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
