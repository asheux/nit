//! Saturation alarm — emits one Warning per cooldown window when active
//! signal count exceeds THRESHOLD.

use super::{ObservedEmission, Observer};
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
    let window_start = sub.current_generation().saturating_sub(COOLDOWN_GENS);
    if super::recent_global_warning(sub, "observer:global_heat", window_start) {
        return Vec::new();
    }

    vec![ObservedEmission::new(
        SignalKind::Warning,
        SignalTarget::Global,
        serde_json::json!({
            "reason": "global_heat",
            "signal_count": n,
            "threshold": THRESHOLD,
        }),
    )]
}
