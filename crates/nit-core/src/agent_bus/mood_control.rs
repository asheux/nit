use crate::mood::{Mood, MOOD_OVERRIDE_LOCK_GENS};
use crate::state::AppState;
use crate::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};

pub(super) fn handle_set_mood(state: &mut AppState, mood: Mood, source: &str) {
    let from = state.substrate.mood;
    let current_gen = state.substrate.current_generation();
    state.substrate.mood = mood;
    state.substrate.mood_override_until_gen = current_gen.saturating_add(MOOD_OVERRIDE_LOCK_GENS);

    let posted_by = "mood".to_string();
    let id = state.substrate.next_signal_id(&posted_by);
    let posted_at_gen = state.substrate.current_generation();
    state.substrate.emit_signal(Signal {
        id,
        kind: SignalKind::Warning,
        posted_by,
        posted_at_gen,
        target: SignalTarget::Global,
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::json!({
            "reason": "mood_manual_override",
            "from": format!("{from:?}").to_lowercase(),
            "to": format!("{mood:?}").to_lowercase(),
            "source": source,
            "lock_until_gen": state.substrate.mood_override_until_gen,
        }),
    });
    let _ = state.substrate.save(&state.workspace_root);
}
