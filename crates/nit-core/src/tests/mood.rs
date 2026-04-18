use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::agent_bus::AgentBusEvent;
use crate::metabolism::{tick, tick_interval_for};
use crate::mood::{auto_transition, Mood};
use crate::state::AppState;
use crate::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};
use crate::Buffer;

fn temp_dir(label: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("nit-test-{label}-{now}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_state(label: &str) -> AppState {
    let dir = temp_dir(label);
    let editor = Buffer::from_str("editor", "", None);
    let notes = Buffer::from_str("notes", "", None);
    AppState::new(dir, editor, notes)
}

fn inject_warning(state: &mut AppState, posted_by: &str, counter: u64) {
    let posted_at_gen = state.substrate.generation;
    let id = format!("{posted_at_gen}-{posted_by}-{counter}");
    state.substrate.emit_signal(Signal {
        id,
        kind: SignalKind::Warning,
        posted_by: posted_by.into(),
        posted_at_gen,
        target: SignalTarget::Agent {
            agent_id: posted_by.into(),
        },
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::Value::Null,
    });
}

#[test]
fn default_mood_is_consolidation() {
    let state = SubstrateState::default();
    assert_eq!(state.mood, Mood::Consolidation);
    assert_eq!(state.mood_override_until_gen, 0);
    assert_eq!(state.mood_quiet_streak, 0);
}

#[test]
fn mood_roundtrips_through_serde() {
    let mut state = SubstrateState::default();
    state.mood = Mood::Defensive;
    state.mood_override_until_gen = 42;
    state.mood_quiet_streak = 7;

    let json = serde_json::to_string(&state).unwrap();
    let restored: SubstrateState = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.mood, Mood::Defensive);
    assert_eq!(restored.mood_override_until_gen, 42);
    assert_eq!(restored.mood_quiet_streak, 7);
}

#[test]
fn legacy_state_json_without_mood_loads_as_consolidation() {
    let root = temp_dir("mood-legacy-load");
    let dir = root.join(".nit").join("substrate");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{"generation":3,"signals":{},"claims":{},"observations":[],"assumptions":{}}"#,
    )
    .unwrap();

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.generation, 3);
    assert_eq!(loaded.mood, Mood::Consolidation);
    assert_eq!(loaded.mood_override_until_gen, 0);
    assert_eq!(loaded.mood_quiet_streak, 0);
}

#[test]
fn auto_transition_c_to_defensive_on_pressure() {
    // At pressure >= 8 from Consolidation, move to Defensive.
    assert_eq!(
        auto_transition(Mood::Consolidation, 8, 0),
        Some(Mood::Defensive)
    );
    // Pressure 7 isn't enough.
    assert_eq!(auto_transition(Mood::Consolidation, 7, 0), None);
}

#[test]
fn auto_transition_defensive_to_c_hysteresis() {
    // Defensive stays put at pressure 6.
    assert_eq!(auto_transition(Mood::Defensive, 6, 0), None);
    // Drops back to Consolidation when pressure <= 4.
    assert_eq!(
        auto_transition(Mood::Defensive, 4, 0),
        Some(Mood::Consolidation)
    );
}

#[test]
fn auto_transition_does_not_thrash() {
    // Oscillating pressure 7-9 stays Defensive.
    for p in [7usize, 8, 9, 7, 8, 9] {
        assert_eq!(auto_transition(Mood::Defensive, p, 0), None);
    }
    // Pressure 5-6 also stays Defensive.
    for p in [5usize, 6] {
        assert_eq!(auto_transition(Mood::Defensive, p, 0), None);
    }
    // Only drops when <= 4.
    assert_eq!(
        auto_transition(Mood::Defensive, 4, 0),
        Some(Mood::Consolidation)
    );
}

#[test]
fn auto_transition_c_to_exploration_requires_streak() {
    // Zero pressure without streak is not enough.
    assert_eq!(auto_transition(Mood::Consolidation, 0, 0), None);
    // Streak >= 3 with low pressure unlocks Exploration.
    assert_eq!(
        auto_transition(Mood::Consolidation, 0, 3),
        Some(Mood::Exploration)
    );
}

#[test]
fn manual_override_blocks_auto_transition() {
    let mut state = test_state("mood-override-blocks");
    state.substrate.generation = 1;
    state.substrate.mood = Mood::Defensive;
    state.substrate.mood_override_until_gen = state.substrate.generation + 20;

    // No warnings at all (pressure=0). Auto-rule would drop Defensive to
    // Consolidation, but the override lock must hold.
    for _ in 0..5 {
        tick(&mut state);
    }
    assert_eq!(state.substrate.mood, Mood::Defensive);
}

#[test]
fn metabolism_reads_mood_adjusted_interval() {
    assert_eq!(tick_interval_for(Mood::Defensive), Duration::from_secs(3));
    assert_eq!(
        tick_interval_for(Mood::Exploration),
        Duration::from_secs(10)
    );
    assert_eq!(
        tick_interval_for(Mood::Consolidation),
        Duration::from_secs(5)
    );
}

#[test]
fn observer_repeat_failure_uses_mood_threshold() {
    use crate::observers::repeat_failure;

    // Defensive mood: threshold 1 — a single Warning is enough.
    let mut state_d = test_state("mood-obs-defensive");
    state_d.substrate.mood = Mood::Defensive;
    inject_warning(&mut state_d, "a1", 0);
    let emissions = (repeat_failure::OBSERVER.run)(&state_d);
    assert_eq!(
        emissions.len(),
        1,
        "defensive mood with 1 warning should trigger HelpNeeded"
    );
    assert_eq!(emissions[0].kind, SignalKind::HelpNeeded);

    // Exploration mood: threshold 3 — 2 warnings should NOT trigger.
    let mut state_e = test_state("mood-obs-exploration");
    state_e.substrate.mood = Mood::Exploration;
    inject_warning(&mut state_e, "a1", 0);
    inject_warning(&mut state_e, "a1", 1);
    let emissions = (repeat_failure::OBSERVER.run)(&state_e);
    assert!(
        emissions.is_empty(),
        "exploration mood with 2 warnings should be silent"
    );
}

#[test]
fn set_mood_event_applies_and_sets_override_lock() {
    let mut state = test_state("mood-set-event-apply");
    state.substrate.generation = 5;
    assert_eq!(state.substrate.mood, Mood::Consolidation);

    let event = AgentBusEvent::SetMood {
        mood: Mood::Defensive,
        source: "user".into(),
    };
    event.apply(&mut state);

    assert_eq!(state.substrate.mood, Mood::Defensive);
    assert!(state.substrate.mood_override_until_gen > 0);
    assert_eq!(state.substrate.mood_override_until_gen, 5 + 20);

    let shift_signal = state
        .substrate
        .signals
        .values()
        .find(|s| {
            s.posted_by == "mood"
                && s.payload.get("reason").and_then(|v| v.as_str())
                    == Some("mood_manual_override")
        })
        .expect("expected mood_manual_override signal");
    assert_eq!(
        shift_signal.payload.get("source").and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        shift_signal.payload.get("to").and_then(|v| v.as_str()),
        Some("defensive")
    );
}
