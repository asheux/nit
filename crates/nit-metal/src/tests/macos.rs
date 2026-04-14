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

const M4_MAX: &str = "Apple M4 Max";
const M4_PRO: &str = "Apple M4 Pro";

fn test_cache_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nit-metal-policy-tests-{}-{}",
        label,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn sample_fsm_payload(
    state_count: u32,
    alphabet_size: u32,
    population_size: usize,
) -> BatchPayload {
    BatchPayload::Fsm(FsmBatch {
        states: state_count,
        alphabet: alphabet_size,
        starts: vec![0; population_size],
        outputs: vec![0; state_count as usize * population_size],
        transitions: vec![0; state_count as usize * alphabet_size as usize * population_size],
    })
}

fn make_cache_entry(device: &str, sig: &str, batch_cap: usize, depth: usize) -> PolicyCacheEntry {
    PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name: device.into(),
        payload_signature: sig.into(),
        matches_per_batch_cap: batch_cap,
        inflight_batches: depth,
    }
}

#[test]
fn max_devices_use_tuned_fsm_batch_limit() {
    let workload = sample_fsm_payload(4, 2, 1);
    assert_eq!(preferred_base_limit(M4_MAX, &workload), 262_144);
    assert_eq!(preferred_inflight_batches(M4_MAX), 4);
}

#[test]
fn non_max_devices_keep_conservative_limits() {
    let fsm_workload = sample_fsm_payload(4, 2, 1);
    let ca_workload = BatchPayload::Ca(CaBatch {
        symbols: 2,
        two_r: 2,
        steps: 32,
        rule_table_len: 8,
        rule_tables: vec![0; 8],
    });
    let tm_workload = BatchPayload::Tm(TmBatch {
        states: 2,
        symbols: 2,
        blank: 0,
        max_steps: 64,
        start_states: vec![0],
        transitions: vec![],
    });
    assert_eq!(preferred_base_limit(M4_PRO, &fsm_workload), 131_072);
    assert_eq!(preferred_base_limit(M4_PRO, &ca_workload), 65_536);
    assert_eq!(preferred_base_limit(M4_PRO, &tm_workload), 32_768);
    assert_eq!(preferred_inflight_batches(M4_PRO), 3);
}

#[test]
fn tm_shader_key_widens_beyond_default() {
    let within_default = ShaderKey::for_tm(64);
    assert_eq!(within_default.tm_max_width, TM_MAX_WIDTH);

    let at_boundary = ShaderKey::for_tm(TM_MAX_WIDTH - 1);
    assert_eq!(at_boundary.tm_max_width, TM_MAX_WIDTH);

    let exceeds_default = ShaderKey::for_tm(TM_MAX_WIDTH);
    assert_eq!(exceeds_default.tm_max_width, TM_MAX_WIDTH + 1);
}

#[test]
fn fsm_shader_key_widens_beyond_default() {
    let within_default = ShaderKey::for_fsm(3);
    assert_eq!(within_default.fsm_max_states, FSM_MAX_STATES);

    let at_boundary = ShaderKey::for_fsm(FSM_MAX_STATES);
    assert_eq!(at_boundary.fsm_max_states, FSM_MAX_STATES);

    let exceeds_default = ShaderKey::for_fsm(FSM_MAX_STATES + 1);
    assert_eq!(exceeds_default.fsm_max_states, FSM_MAX_STATES + 1);
}

#[test]
fn fsm_payload_sets_shader_key_states() {
    let standard_workload = sample_fsm_payload(3, 2, 1);
    let resolved = ShaderKey::for_payload(&standard_workload);
    assert_eq!(resolved.fsm_max_states, FSM_MAX_STATES);

    let oversized_workload = sample_fsm_payload(FSM_MAX_STATES + 2, 2, 1);
    let widened = ShaderKey::for_payload(&oversized_workload);
    assert_eq!(widened.fsm_max_states, FSM_MAX_STATES + 2);
}

#[test]
fn policy_cache_round_trips() {
    let workload = sample_fsm_payload(4, 2, 4);
    let sig = payload_signature(&workload);
    let cache_dir = test_cache_dir("roundtrip");
    let original = make_cache_entry(M4_MAX, &sig, 262_144, 4);
    persist_cached_policy_from_dir(&cache_dir, &original);
    let restored =
        load_cached_policy_from_dir(&cache_dir, M4_MAX, &sig).expect("cache entry should load");
    assert_eq!(restored, original);
    let _ = fs::remove_dir_all(cache_dir);
}

#[test]
fn policy_cache_snapshot_and_clear() {
    let temp_root = test_cache_dir("snapshot");
    let cache_path = temp_root.join("games").join("metal-policy");
    let fsm_entry = make_cache_entry(M4_MAX, "fsm_s4_a2_n51924_static1mib", 262_144, 4);
    let tm_entry = make_cache_entry(M4_MAX, "tm_s2_sym2_steps64_n128_static1mib", 32_768, 4);
    persist_cached_policy_from_dir(&cache_path, &fsm_entry);
    persist_cached_policy_from_dir(&cache_path, &tm_entry);

    let initial_snapshot = snapshot_policy_cache_from_dir(&cache_path).expect("snapshot");
    assert_eq!(
        initial_snapshot.root.as_deref(),
        Some(cache_path.to_string_lossy().as_ref())
    );
    assert_eq!(initial_snapshot.entries.len(), 2);
    assert!(initial_snapshot
        .entries
        .iter()
        .any(|e| e.payload_signature == fsm_entry.payload_signature));

    let first_entry_path = Path::new(&initial_snapshot.entries[0].path).to_path_buf();
    assert!(clear_policy_cache_entry_in_root(&cache_path, &first_entry_path).expect("clear entry"));

    let after_single_clear =
        snapshot_policy_cache_from_dir(&cache_path).expect("snapshot after clear");
    assert_eq!(after_single_clear.entries.len(), 1);

    let purged_count = clear_policy_cache_in_root(&cache_path).expect("clear all");
    assert_eq!(purged_count, 1);
    let after_full_clear =
        snapshot_policy_cache_from_dir(&cache_path).expect("snapshot after clear all");
    assert!(after_full_clear.entries.is_empty());
}
