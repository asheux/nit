//! Phase 9 default cover for the multiway roster selectors carried on
//! `AgentsState`. The Mood/Mode selectors persist across sessions, so their
//! defaults are part of the saved-state contract: a fresh state must start on
//! the balanced, linear path, and the transient per-dispatch override must
//! start empty. Exercised through the public `nit_core::` re-export surface.

use nit_core::{AgentsState, MultiwaySearchMood};

#[test]
fn agents_state_starts_on_balanced_linear_multiway_defaults() {
    let state = AgentsState::default();

    assert_eq!(state.multiway_default_mood, MultiwaySearchMood::Balanced);
    assert!(!state.multiway_default_mode_on);
    assert_eq!(state.pending_multiway_mood, None);
}

#[test]
fn multiway_search_mood_round_trips_snake_case() {
    let cases = [
        (MultiwaySearchMood::Explore, "\"explore\""),
        (MultiwaySearchMood::Balanced, "\"balanced\""),
        (MultiwaySearchMood::Exploit, "\"exploit\""),
    ];
    for (mood, wire) in cases {
        let json = serde_json::to_string(&mood).expect("serialize MultiwaySearchMood");
        assert_eq!(json, wire);
        let back: MultiwaySearchMood =
            serde_json::from_str(&json).expect("deserialize MultiwaySearchMood");
        assert_eq!(back, mood);
    }

    assert_eq!(MultiwaySearchMood::default(), MultiwaySearchMood::Balanced);
}
