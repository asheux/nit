//! End-to-end coverage for the macOS Metal backend. Device-tier heuristics,
//! shader-key widening, payload signatures, input sanitisation, and the
//! on-disk policy cache each live in their own `#[test]` below with shared
//! fixtures at the top.

use crate::macos::cache::{
    clear_policy_cache_entry_in_root, clear_policy_cache_in_root, load_cached_policy_from_dir,
    persist_cached_policy_from_dir, sanitize_cache_component, snapshot_policy_cache_from_dir,
    PolicyCacheEntry, POLICY_CACHE_SCHEMA_VERSION,
};
use crate::macos::device::{apple_tier, AppleTier};
use crate::macos::policy::{payload_signature, preferred_base_limit, preferred_inflight_batches};
use crate::macos::shader::ShaderKey;
use crate::{
    BatchPayload, CaBatch, FsmBatch, TmBatch, CA_MAX_WINDOW, FSM_MAX_STATES, TM_MAX_WIDTH,
};
use std::{fs, path::Path, path::PathBuf};

// ---------------------------------------------------------------------------
// Device names covering the tier-detection matrix.
// ---------------------------------------------------------------------------

const DEVICE_M4_MAX: &str = "Apple M4 Max";
const DEVICE_M4_PRO: &str = "Apple M4 Pro";
const DEVICE_M2_ULTRA: &str = "Apple M2 Ultra";
const DEVICE_M1_BASE: &str = "Apple M1";

// ---------------------------------------------------------------------------
// Payload builders — keep each variant isolated so a future field addition
// only perturbs one helper.
// ---------------------------------------------------------------------------

fn payload_fsm(states: u32, alphabet: u32, population: usize) -> BatchPayload {
    BatchPayload::Fsm(FsmBatch {
        states,
        alphabet,
        starts: vec![0; population],
        outputs: vec![0; states as usize * population],
        transitions: vec![0; states as usize * alphabet as usize * population],
    })
}

fn payload_ca() -> BatchPayload {
    BatchPayload::Ca(CaBatch {
        symbols: 2,
        two_r: 2,
        steps: 32,
        rule_table_len: 8,
        rule_tables: vec![0; 8],
    })
}

fn payload_tm() -> BatchPayload {
    BatchPayload::Tm(TmBatch {
        states: 2,
        symbols: 2,
        blank: 0,
        max_steps: 64,
        start_states: vec![0],
        transitions: vec![],
    })
}

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nit-metal-policy-tests-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn make_entry(device: &str, sig: &str, cap: usize, depth: usize) -> PolicyCacheEntry {
    PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name: device.into(),
        payload_signature: sig.into(),
        matches_per_batch_cap: cap,
        inflight_batches: depth,
    }
}

// ---------------------------------------------------------------------------
// Tier detection
// ---------------------------------------------------------------------------

#[test]
fn apple_tier_maps_every_device_family() {
    for (name, want) in [
        (DEVICE_M2_ULTRA, AppleTier::Ultra),
        (DEVICE_M4_MAX, AppleTier::Max),
        (DEVICE_M4_PRO, AppleTier::Pro),
        (DEVICE_M1_BASE, AppleTier::Base),
    ] {
        assert_eq!(apple_tier(name), want, "tier for {name}");
    }
}

#[test]
fn high_tier_devices_double_fsm_batch_ceiling() {
    let payload = payload_fsm(4, 2, 1);
    for device in [DEVICE_M4_MAX, DEVICE_M2_ULTRA] {
        assert_eq!(preferred_base_limit(device, &payload), 262_144, "{device}");
    }
    assert_eq!(preferred_inflight_batches(DEVICE_M4_MAX), 4);
    assert_eq!(preferred_inflight_batches(DEVICE_M2_ULTRA), 5);
}

#[test]
fn conservative_devices_shrink_batch_ceilings() {
    let expected = [
        (payload_fsm(4, 2, 1), 131_072usize),
        (payload_ca(), 65_536),
        (payload_tm(), 32_768),
    ];
    for (payload, want) in &expected {
        assert_eq!(preferred_base_limit(DEVICE_M4_PRO, payload), *want);
    }
    assert_eq!(preferred_inflight_batches(DEVICE_M4_PRO), 3);
    assert_eq!(preferred_inflight_batches(DEVICE_M1_BASE), 2);
}

// ---------------------------------------------------------------------------
// Shader key widening — one `for_*` constructor per kernel family.
// ---------------------------------------------------------------------------

#[test]
fn shader_key_stays_at_default_below_compiled_bound() {
    assert_eq!(ShaderKey::for_tm(64).tm_max_width, TM_MAX_WIDTH);
    assert_eq!(
        ShaderKey::for_tm(TM_MAX_WIDTH - 1).tm_max_width,
        TM_MAX_WIDTH
    );
    assert_eq!(ShaderKey::for_fsm(3).fsm_max_states, FSM_MAX_STATES);
    assert_eq!(
        ShaderKey::for_fsm(FSM_MAX_STATES).fsm_max_states,
        FSM_MAX_STATES
    );
    assert_eq!(ShaderKey::for_ca(4).ca_max_window, CA_MAX_WINDOW);
}

#[test]
fn shader_key_widens_above_compiled_bound() {
    assert_eq!(
        ShaderKey::for_tm(TM_MAX_WIDTH).tm_max_width,
        TM_MAX_WIDTH + 1
    );
    assert_eq!(
        ShaderKey::for_fsm(FSM_MAX_STATES + 1).fsm_max_states,
        FSM_MAX_STATES + 1
    );
    assert_eq!(
        ShaderKey::for_ca(CA_MAX_WINDOW + 5).ca_max_window,
        CA_MAX_WINDOW + 5
    );
}

#[test]
fn fsm_payload_shader_key_tracks_population_states() {
    let within = ShaderKey::for_payload(&payload_fsm(3, 2, 1));
    assert_eq!(within.fsm_max_states, FSM_MAX_STATES);

    let widened = ShaderKey::for_payload(&payload_fsm(FSM_MAX_STATES + 2, 2, 1));
    assert_eq!(widened.fsm_max_states, FSM_MAX_STATES + 2);
}

// ---------------------------------------------------------------------------
// Payload signatures + cache-key sanitization
// ---------------------------------------------------------------------------

#[test]
fn payload_signature_embeds_variant_prefix_and_memory_bucket() {
    let cases: &[(BatchPayload, &str)] = &[
        (payload_fsm(4, 2, 8), "fsm_s4_a2_n8_"),
        (payload_ca(), "ca_sym2_twor2_steps32_table8_"),
        (payload_tm(), "tm_s2_sym2_steps64_"),
    ];
    for (payload, prefix) in cases {
        let sig = payload_signature(payload);
        assert!(sig.starts_with(prefix), "signature {sig} missing {prefix}");
        assert!(sig.ends_with("mib"), "signature {sig} missing bucket");
    }
}

#[test]
fn sanitize_cache_component_normalizes_punctuation_and_case() {
    let cases = [
        ("Apple M4 Max", "apple_m4_max"),
        ("  leading__and--trailing  ", "leading_and_trailing"),
        ("", ""),
        ("α β 42", "42"),
    ];
    for (input, want) in cases {
        assert_eq!(sanitize_cache_component(input), want, "input={input:?}");
    }
}

// ---------------------------------------------------------------------------
// Policy cache I/O
// ---------------------------------------------------------------------------

#[test]
fn policy_cache_round_trips() {
    let sig = payload_signature(&payload_fsm(4, 2, 4));
    let dir = scratch_dir("roundtrip");
    let original = make_entry(DEVICE_M4_MAX, &sig, 262_144, 4);
    persist_cached_policy_from_dir(&dir, &original);

    let restored =
        load_cached_policy_from_dir(&dir, DEVICE_M4_MAX, &sig).expect("cache entry should load");
    assert_eq!(restored, original);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn policy_cache_snapshot_and_clear() {
    let temp_root = scratch_dir("snapshot");
    let cache_path = temp_root.join("games").join("metal-policy");
    let seeded = [
        make_entry(DEVICE_M4_MAX, "fsm_s4_a2_n51924_static1mib", 262_144, 4),
        make_entry(
            DEVICE_M4_MAX,
            "tm_s2_sym2_steps64_n128_static1mib",
            32_768,
            4,
        ),
    ];
    for entry in &seeded {
        persist_cached_policy_from_dir(&cache_path, entry);
    }

    let initial = snapshot_policy_cache_from_dir(&cache_path).expect("snapshot");
    assert_eq!(
        initial.root.as_deref(),
        Some(cache_path.to_string_lossy().as_ref())
    );
    assert_eq!(initial.entries.len(), seeded.len());
    assert!(initial
        .entries
        .iter()
        .any(|e| e.payload_signature == seeded[0].payload_signature));

    let first = Path::new(&initial.entries[0].path).to_path_buf();
    assert!(clear_policy_cache_entry_in_root(&cache_path, &first).expect("clear entry"));

    let after_single = snapshot_policy_cache_from_dir(&cache_path).expect("post-clear snapshot");
    assert_eq!(after_single.entries.len(), 1);

    let purged = clear_policy_cache_in_root(&cache_path).expect("clear all");
    assert_eq!(purged, 1);
    let after_full = snapshot_policy_cache_from_dir(&cache_path).expect("post-full-clear snapshot");
    assert!(after_full.entries.is_empty());

    let _ = fs::remove_dir_all(temp_root);
}

#[test]
fn clear_refuses_paths_outside_cache_root() {
    let cache_path = scratch_dir("escape-guard");
    fs::create_dir_all(&cache_path).expect("mkdir");
    let outside = std::env::temp_dir().join("nit-metal-policy-outside.json");
    let result = clear_policy_cache_entry_in_root(&cache_path, &outside);
    assert!(
        result.is_err(),
        "path traversal outside cache_root must be rejected"
    );
    let _ = fs::remove_dir_all(cache_path);
}
