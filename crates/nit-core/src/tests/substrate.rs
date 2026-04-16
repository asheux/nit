use super::*;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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

#[test]
fn default_state_has_zero_generation() {
    let state = SubstrateState::default();
    assert_eq!(state.current_generation(), 0);
    assert!(state.signals().is_empty());
    assert!(state.claims().is_empty());
    assert!(state.observations().is_empty());
}

#[test]
fn advance_generation_is_monotonic() {
    let mut state = SubstrateState::new();
    assert_eq!(state.advance_generation(), 1);
    assert_eq!(state.advance_generation(), 2);
    assert_eq!(state.advance_generation(), 3);
    assert_eq!(state.current_generation(), 3);
}

#[test]
fn round_trip_serialization() {
    let mut state = SubstrateState::new();
    state.advance_generation();
    state.advance_generation();
    let json = serde_json::to_string(&state).unwrap();
    let restored: SubstrateState = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.generation, 2);
}

#[test]
fn save_then_load_round_trip() {
    let root = temp_dir("substrate-roundtrip");
    let state = SubstrateState::new();
    state.save(&root).unwrap();

    let expected_path = root.join(".nit").join("substrate").join("state.json");
    assert!(expected_path.exists(), "state file should exist after save");

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.generation, state.generation);

    let mut next = loaded;
    next.advance_generation();
    next.save(&root).unwrap();
    let reloaded = SubstrateState::load(&root);
    assert_eq!(reloaded.generation, 1);
}

#[test]
fn load_from_missing_dir_yields_default() {
    let root = temp_dir("substrate-missing");
    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.current_generation(), 0);
    assert!(loaded.signals().is_empty());
    assert!(loaded.claims().is_empty());
    assert!(loaded.observations().is_empty());
}

#[test]
fn load_from_corrupt_file_yields_default() {
    let root = temp_dir("substrate-corrupt");
    let dir = root.join(".nit").join("substrate");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("state.json"), "not json {{").unwrap();

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.current_generation(), 0);
    assert!(loaded.signals().is_empty());
    assert!(loaded.claims().is_empty());
    assert!(loaded.observations().is_empty());
}

fn mk_signal(id: &str, kind: SignalKind, posted_at_gen: u64, target: SignalTarget) -> Signal {
    Signal {
        id: id.into(),
        kind,
        posted_by: "agent-a".into(),
        posted_at_gen,
        target,
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::Value::Null,
    }
}

#[test]
fn signal_round_trip_serialization() {
    let cases = vec![
        mk_signal(
            "s-warn",
            SignalKind::Warning,
            1,
            SignalTarget::File {
                path: PathBuf::from("src/lib.rs"),
            },
        ),
        mk_signal(
            "s-help",
            SignalKind::HelpNeeded,
            2,
            SignalTarget::Agent {
                agent_id: "agent-b".into(),
            },
        ),
        mk_signal("s-done", SignalKind::DoneMarker, 3, SignalTarget::Global),
    ];

    for original in cases {
        let json = serde_json::to_string(&original).unwrap();
        let restored: Signal = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.kind, original.kind);
        assert_eq!(restored.posted_by, original.posted_by);
        assert_eq!(restored.posted_at_gen, original.posted_at_gen);
        assert_eq!(restored.target, original.target);
        assert_eq!(restored.initial_strength, original.initial_strength);
        assert_eq!(restored.payload, original.payload);
    }
}

#[test]
fn decay_monotonic_lazy() {
    let signal = mk_signal("s", SignalKind::DoneMarker, 0, SignalTarget::Global);
    let s0 = signal.effective_strength(0);
    let s1 = signal.effective_strength(1);
    let s2 = signal.effective_strength(2);
    let s5 = signal.effective_strength(5);
    assert_eq!(s0, 1.0);
    assert!(s1 < s0);
    assert!(s2 < s1);
    assert!(s5 < s2);
}

#[test]
fn decay_rate_varies_by_kind() {
    let done = mk_signal("d", SignalKind::DoneMarker, 0, SignalTarget::Global);
    let help = mk_signal("h", SignalKind::HelpNeeded, 0, SignalTarget::Global);
    assert!(done.effective_strength(3) > help.effective_strength(3));
}

#[test]
fn prune_removes_below_threshold() {
    let mut state = SubstrateState::new();
    let signal = mk_signal("s", SignalKind::HelpNeeded, 0, SignalTarget::Global);
    state.emit_signal(signal);
    // HelpNeeded decay rate is 0.5; after 6 gens, 0.5^6 ~= 0.0156 < 0.05 threshold.
    for _ in 0..6 {
        state.advance_generation();
    }
    let removed = state.prune_signals_below(SubstrateState::DEFAULT_PRUNE_THRESHOLD);
    assert!(removed >= 1);
    assert!(state.signals.is_empty());
}

#[test]
fn signals_iter_yields_effective_strength() {
    let mut state = SubstrateState::new();
    let signal = mk_signal("s", SignalKind::Warning, 0, SignalTarget::Global);
    state.emit_signal(signal);
    state.advance_generation();
    state.advance_generation();
    let (found, strength) = state.signals_iter().next().unwrap();
    assert_eq!(strength, found.effective_strength(state.current_generation()));
}

#[test]
fn signals_by_kind_filter() {
    let mut state = SubstrateState::new();
    state.emit_signal(mk_signal(
        "w",
        SignalKind::Warning,
        0,
        SignalTarget::Global,
    ));
    state.emit_signal(mk_signal("l", SignalKind::Lead, 0, SignalTarget::Global));
    let warnings: Vec<_> = state.signals_by_kind(SignalKind::Warning).collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].0.kind, SignalKind::Warning);
}

#[test]
fn signals_by_target_file_filter() {
    let mut state = SubstrateState::new();
    let target_a = SignalTarget::File {
        path: PathBuf::from("a.rs"),
    };
    let target_b = SignalTarget::File {
        path: PathBuf::from("b.rs"),
    };
    state.emit_signal(mk_signal("a", SignalKind::Lead, 0, target_a.clone()));
    state.emit_signal(mk_signal("b", SignalKind::Lead, 0, target_b));
    let matched: Vec<_> = state.signals_by_target(&target_a).collect();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].0.id, "a");
}

#[test]
fn tolerant_load_of_phase1_empty_signals() {
    let root = temp_dir("substrate-phase1");
    let dir = root.join(".nit").join("substrate");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{"generation":3,"signals":{},"claims":{},"observations":[]}"#,
    )
    .unwrap();

    let loaded = SubstrateState::load(&root);
    assert_eq!(loaded.generation, 3);
    assert!(loaded.signals.is_empty());
    assert_eq!(loaded.signal_counter, 0);
}

#[test]
fn next_signal_id_format_and_monotonic() {
    let mut state = SubstrateState::new();
    state.generation = 5;
    let first = state.next_signal_id("agent-a");
    let second = state.next_signal_id("agent-a");
    assert_eq!(first, "5-agent-a-0");
    assert_eq!(second, "5-agent-a-1");
    assert_eq!(state.signal_counter, 2);
}
