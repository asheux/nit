use super::{
    clear_policy_cache_entry_in_root, clear_policy_cache_in_root, load_cached_policy_from_dir,
    payload_signature, persist_cached_policy_from_dir, preferred_base_limit,
    preferred_inflight_batches, snapshot_policy_cache_from_dir, PolicyCacheEntry, ShaderKey,
    POLICY_CACHE_SCHEMA_VERSION,
};
use crate::{BatchPayload, CaBatch, FsmBatch, TmBatch, FSM_MAX_STATES, TM_MAX_WIDTH};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn temp_cache_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "nit-metal-policy-tests-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    root
}

#[test]
fn max_devices_use_tuned_fsm_batch_limit() {
    let payload = BatchPayload::Fsm(FsmBatch {
        states: 4,
        alphabet: 2,
        starts: vec![0],
        outputs: vec![0; 4],
        transitions: vec![0; 8],
    });
    assert_eq!(preferred_base_limit("Apple M4 Max", &payload), 262_144);
    assert_eq!(preferred_inflight_batches("Apple M4 Max"), 4);
}

#[test]
fn non_max_devices_keep_conservative_limits() {
    let fsm_payload = BatchPayload::Fsm(FsmBatch {
        states: 4,
        alphabet: 2,
        starts: vec![0],
        outputs: vec![0; 4],
        transitions: vec![0; 8],
    });
    let ca_payload = BatchPayload::Ca(CaBatch {
        symbols: 2,
        two_r: 2,
        steps: 32,
        rule_table_len: 8,
        rule_tables: vec![0; 8],
    });
    let tm_payload = BatchPayload::Tm(TmBatch {
        states: 2,
        symbols: 2,
        blank: 0,
        max_steps: 64,
        start_states: vec![0],
        transitions: vec![],
    });
    assert_eq!(preferred_base_limit("Apple M4 Pro", &fsm_payload), 131_072);
    assert_eq!(preferred_base_limit("Apple M4 Pro", &ca_payload), 65_536);
    assert_eq!(preferred_base_limit("Apple M4 Pro", &tm_payload), 32_768);
    assert_eq!(preferred_inflight_batches("Apple M4 Pro"), 3);
}

#[test]
fn tm_shader_key_reuses_default_width_until_needed() {
    assert_eq!(ShaderKey::for_tm(64).tm_max_width, TM_MAX_WIDTH);
    assert_eq!(
        ShaderKey::for_tm(TM_MAX_WIDTH - 1).tm_max_width,
        TM_MAX_WIDTH
    );
    assert_eq!(
        ShaderKey::for_tm(TM_MAX_WIDTH).tm_max_width,
        TM_MAX_WIDTH + 1
    );
}

#[test]
fn fsm_shader_key_reuses_default_states_until_needed() {
    assert_eq!(ShaderKey::for_fsm(3).fsm_max_states, FSM_MAX_STATES);
    assert_eq!(
        ShaderKey::for_fsm(FSM_MAX_STATES).fsm_max_states,
        FSM_MAX_STATES
    );
    assert_eq!(
        ShaderKey::for_fsm(FSM_MAX_STATES + 1).fsm_max_states,
        FSM_MAX_STATES + 1
    );
}

#[test]
fn fsm_payload_sets_shader_key_states() {
    let payload = BatchPayload::Fsm(FsmBatch {
        states: 3,
        alphabet: 2,
        starts: vec![0],
        outputs: vec![0; 3],
        transitions: vec![0; 6],
    });
    let key = ShaderKey::for_payload(&payload);
    assert_eq!(key.fsm_max_states, FSM_MAX_STATES);

    let large = BatchPayload::Fsm(FsmBatch {
        states: FSM_MAX_STATES + 2,
        alphabet: 2,
        starts: vec![0],
        outputs: vec![0; (FSM_MAX_STATES + 2) as usize],
        transitions: vec![0; (FSM_MAX_STATES + 2) as usize * 2],
    });
    let key = ShaderKey::for_payload(&large);
    assert_eq!(key.fsm_max_states, FSM_MAX_STATES + 2);
}

#[test]
fn policy_cache_round_trips_by_device_and_signature() {
    let payload = BatchPayload::Fsm(FsmBatch {
        states: 4,
        alphabet: 2,
        starts: vec![0, 1, 2, 3],
        outputs: vec![0; 16],
        transitions: vec![0; 32],
    });
    let signature = payload_signature(&payload);
    let root = temp_cache_root("roundtrip");
    let entry = PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name: "Apple M4 Max".into(),
        payload_signature: signature.clone(),
        matches_per_batch_cap: 262_144,
        inflight_batches: 4,
    };
    persist_cached_policy_from_dir(&root, &entry);
    let loaded = load_cached_policy_from_dir(&root, "Apple M4 Max", &signature)
        .expect("cache entry should load");
    assert_eq!(loaded, entry);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn policy_cache_snapshot_and_clear_work_from_directory() {
    let root = temp_cache_root("snapshot");
    let path = root.join("games").join("metal-policy");
    let entry_a = PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name: "Apple M4 Max".into(),
        payload_signature: "fsm_s4_a2_n51924_static1mib".into(),
        matches_per_batch_cap: 262_144,
        inflight_batches: 4,
    };
    let entry_b = PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name: "Apple M4 Max".into(),
        payload_signature: "tm_s2_sym2_steps64_n128_static1mib".into(),
        matches_per_batch_cap: 32_768,
        inflight_batches: 4,
    };
    persist_cached_policy_from_dir(&path, &entry_a);
    persist_cached_policy_from_dir(&path, &entry_b);

    let snapshot = snapshot_policy_cache_from_dir(&path).expect("snapshot");
    assert_eq!(
        snapshot.root.as_deref(),
        Some(path.to_string_lossy().as_ref())
    );
    assert_eq!(snapshot.entries.len(), 2);
    assert!(snapshot
        .entries
        .iter()
        .any(|entry| entry.payload_signature == entry_a.payload_signature));

    let first_path = Path::new(&snapshot.entries[0].path).to_path_buf();
    assert!(clear_policy_cache_entry_in_root(&path, &first_path).expect("clear entry"));

    let snapshot = snapshot_policy_cache_from_dir(&path).expect("snapshot after clear");
    assert_eq!(snapshot.entries.len(), 1);

    let removed = clear_policy_cache_in_root(&path).expect("clear all");
    assert_eq!(removed, 1);
    let snapshot = snapshot_policy_cache_from_dir(&path).expect("snapshot after clear all");
    assert!(snapshot.entries.is_empty());
}
