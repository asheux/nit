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
