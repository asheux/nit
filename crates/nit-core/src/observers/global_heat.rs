//! Saturation alarm — emits one Warning per cooldown window when the active
//! signal count exceeds `THRESHOLD`. Without the cooldown the arbiter feed
//! would itself become a hot signal source while we are flagging hot signals.

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
    let signal_count = sub.signals.len();
    if signal_count <= THRESHOLD {
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
            "signal_count": signal_count,
            "threshold": THRESHOLD,
        }),
    )]
}
