use crate::{
    BatchEvalConfig, BatchExecutionPolicy, BatchPayload, BatchPolicyCacheEntryInfo,
    BatchPolicyCacheSnapshot, BatchPolicySource, BatchRequest, MatchPair, RecommendedBatchPolicy,
    ScorePair, TmHaltingPair, TmTransitionPacked, CA_MAX_WINDOW, FSM_MAX_STATES, TM_MAX_WIDTH,
};
use metal::{CompileOptions, ComputePipelineState, Device, Library, MTLResourceOptions, MTLSize};
use nit_utils::{fs::write_atomic, hashing::stable_hash_bytes, paths::cache_dir};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::mem::{size_of, size_of_val};
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const SHADER_SOURCE: &str = include_str!("batch_eval.metal");
const POLICY_CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct ShaderKey {
    ca_max_window: u32,
    tm_max_width: u32,
    fsm_max_states: u32,
}

impl ShaderKey {
    fn defaults() -> Self {
        Self {
            ca_max_window: CA_MAX_WINDOW.max(1),
            tm_max_width: TM_MAX_WIDTH.max(1),
            fsm_max_states: FSM_MAX_STATES.max(1),
        }
    }

    fn for_fsm(states: u32) -> Self {
        let required = states.max(1);
        Self {
            ca_max_window: CA_MAX_WINDOW.max(1),
            tm_max_width: TM_MAX_WIDTH.max(1),
            fsm_max_states: if required <= FSM_MAX_STATES.max(1) {
                FSM_MAX_STATES.max(1)
            } else {
                required
            },
        }
    }

    fn for_tm(max_steps: u32) -> Self {
        let required_width = max_steps.saturating_add(1).max(1);
        Self {
            ca_max_window: CA_MAX_WINDOW.max(1),
            tm_max_width: if required_width <= TM_MAX_WIDTH.max(1) {
                TM_MAX_WIDTH.max(1)
            } else {
                required_width
            },
            fsm_max_states: FSM_MAX_STATES.max(1),
        }
    }

    fn for_ca(window: u32) -> Self {
        let required = window.max(1);
        Self {
            ca_max_window: if required <= CA_MAX_WINDOW.max(1) {
                CA_MAX_WINDOW.max(1)
            } else {
                required
            },
            tm_max_width: TM_MAX_WIDTH.max(1),
            fsm_max_states: FSM_MAX_STATES.max(1),
        }
    }

    fn for_payload(payload: &BatchPayload) -> Self {
        match payload {
            BatchPayload::Fsm(batch) => Self::for_fsm(batch.states),
            BatchPayload::Tm(batch) => Self::for_tm(batch.max_steps),
            BatchPayload::Ca(batch) => {
                Self::for_ca(batch.two_r.saturating_mul(batch.steps).saturating_add(1))
            }
        }
    }
}

fn shader_source_for_key(key: ShaderKey) -> String {
    // Metal requires fixed-size arrays. We specialize the kernels by compiling the shader
    // with the requested scratch sizes, caching pipeline states per key.
    format!(
        "#define CA_MAX_WINDOW {}u\n#define TM_MAX_WIDTH {}u\n#define FSM_MAX_STATES {}u\n{}",
        key.ca_max_window, key.tm_max_width, key.fsm_max_states, SHADER_SOURCE
    )
}

#[repr(C)]
#[derive(Copy, Clone)]
struct EvalParams {
    rounds: u32,
    pair_count: u32,
    cc_a: i32,
    cc_b: i32,
    cd_a: i32,
    cd_b: i32,
    dc_a: i32,
    dc_b: i32,
    dd_a: i32,
    dd_b: i32,
    timeout_lose: i32,
    timeout_win: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct FsmParams {
    states: u32,
    alphabet: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct CaParams {
    symbols: u32,
    two_r: u32,
    steps: u32,
    rule_table_len: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct TmParams {
    states: u32,
    symbols: u32,
    blank: u32,
    max_steps: u32,
    transitions_per_strategy: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct MatchPairPod {
    a_idx: u32,
    b_idx: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct ScorePairPod {
    a_total: i64,
    b_total: i64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct TmHaltingPairPod {
    a_all_halted: u32,
    b_all_halted: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct TmTransitionPod {
    write: u32,
    move_dir: u32,
    next: u32,
    pad: u32,
}

struct MetalContext {
    device: Device,
    queue: metal::CommandQueue,
    _library: Library,
    fsm_pipeline: ComputePipelineState,
    ca_pipeline: ComputePipelineState,
    tm_pipeline: ComputePipelineState,
}

enum PreparedPayload {
    Fsm {
        params: FsmParams,
        starts: metal::Buffer,
        outputs: metal::Buffer,
        transitions: metal::Buffer,
    },
    Ca {
        params: CaParams,
        rule_tables: metal::Buffer,
    },
    Tm {
        params: TmParams,
        starts: metal::Buffer,
        transitions: metal::Buffer,
    },
}

pub struct PreparedBatch {
    shader_key: ShaderKey,
    eval: BatchEvalConfig,
    payload: PreparedPayload,
}

pub struct PendingBatch {
    _pair_buffer: metal::Buffer,
    score_buffer: metal::Buffer,
    tm_halting_buffer: Option<metal::Buffer>,
    command_buffer: metal::CommandBuffer,
    pair_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PolicyCacheEntry {
    schema_version: u32,
    device_name: String,
    payload_signature: String,
    matches_per_batch_cap: usize,
    inflight_batches: usize,
}

fn payload_static_bytes(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(payload) => {
            payload.starts.len() * size_of::<u32>()
                + payload.outputs.len() * size_of::<u32>()
                + payload.transitions.len() * size_of::<u32>()
        }
        BatchPayload::Ca(payload) => payload.rule_tables.len() * size_of::<u32>(),
        BatchPayload::Tm(payload) => {
            payload.start_states.len() * size_of::<u32>()
                + payload.transitions.len() * size_of::<TmTransitionPacked>()
        }
    }
}

fn payload_strategy_count(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(payload) => payload.starts.len(),
        BatchPayload::Ca(payload) => {
            let per_strategy = payload.rule_table_len.max(1) as usize;
            payload.rule_tables.len() / per_strategy
        }
        BatchPayload::Tm(payload) => payload.start_states.len(),
    }
}

fn payload_signature(payload: &BatchPayload) -> String {
    let static_mib_bucket = {
        let bytes = payload_static_bytes(payload);
        let mib = bytes.div_ceil(1024 * 1024).max(1);
        mib.next_power_of_two()
    };
    match payload {
        BatchPayload::Fsm(payload) => format!(
            "fsm_s{}_a{}_n{}_static{}mib",
            payload.states,
            payload.alphabet,
            payload.starts.len(),
            static_mib_bucket
        ),
        BatchPayload::Ca(payload) => format!(
            "ca_sym{}_twor{}_steps{}_table{}_n{}_static{}mib",
            payload.symbols,
            payload.two_r,
            payload.steps,
            payload.rule_table_len,
            payload_strategy_count(&BatchPayload::Ca(payload.clone())),
            static_mib_bucket
        ),
        BatchPayload::Tm(payload) => format!(
            "tm_s{}_sym{}_steps{}_n{}_static{}mib",
            payload.states,
            payload.symbols,
            payload.max_steps,
            payload.start_states.len(),
            static_mib_bucket
        ),
    }
}

fn sanitize_cache_component(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    let mut prev_sep = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch.to_ascii_lowercase());
            prev_sep = false;
        } else if !prev_sep {
            sanitized.push('_');
            prev_sep = true;
        }
    }
    sanitized.trim_matches('_').to_string()
}

fn policy_cache_root() -> Option<PathBuf> {
    cache_dir().map(|path| path.join("games").join("metal-policy"))
}

fn policy_cache_key(device_name: &str, payload_signature: &str) -> String {
    let device_slug = sanitize_cache_component(device_name);
    let cache_key = stable_hash_bytes(format!("{device_name}:{payload_signature}").as_bytes());
    format!("{device_slug}_{cache_key}")
}

fn policy_cache_path(root: &Path, device_name: &str, payload_signature: &str) -> PathBuf {
    root.join(format!(
        "{}_v{}.json",
        policy_cache_key(device_name, payload_signature),
        POLICY_CACHE_SCHEMA_VERSION
    ))
}

fn load_cached_policy_from_dir(
    root: &Path,
    device_name: &str,
    payload_signature: &str,
) -> Option<PolicyCacheEntry> {
    let path = policy_cache_path(root, device_name, payload_signature);
    let contents = fs::read(path).ok()?;
    let entry: PolicyCacheEntry = serde_json::from_slice(&contents).ok()?;
    if entry.schema_version != POLICY_CACHE_SCHEMA_VERSION
        || entry.device_name != device_name
        || entry.payload_signature != payload_signature
        || entry.matches_per_batch_cap == 0
        || entry.inflight_batches == 0
    {
        return None;
    }
    Some(entry)
}

fn load_cached_policy(device_name: &str, payload_signature: &str) -> Option<PolicyCacheEntry> {
    let root = policy_cache_root()?;
    load_cached_policy_from_dir(&root, device_name, payload_signature)
}

fn persist_cached_policy_from_dir(root: &Path, entry: &PolicyCacheEntry) {
    if fs::create_dir_all(root).is_err() {
        return;
    }
    let path = policy_cache_path(root, &entry.device_name, &entry.payload_signature);
    let _ = write_atomic(&path, |writer| {
        serde_json::to_writer(writer, entry)
            .map_err(std::io::Error::other)
    });
}

fn persist_cached_policy(entry: &PolicyCacheEntry) {
    let Some(root) = policy_cache_root() else {
        return;
    };
    persist_cached_policy_from_dir(&root, entry);
}

fn snapshot_policy_cache_from_dir(root: &Path) -> Result<BatchPolicyCacheSnapshot, String> {
    let mut snapshot = BatchPolicyCacheSnapshot {
        root: Some(root.to_string_lossy().into_owned()),
        entries: Vec::new(),
    };
    let read_dir = match fs::read_dir(root) {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(snapshot),
        Err(err) => {
            return Err(format!(
                "failed to read Metal policy cache {}: {err}",
                root.display()
            ));
        }
    };
    for dir_entry in read_dir {
        let dir_entry = dir_entry.map_err(|err| {
            format!(
                "failed to enumerate Metal policy cache {}: {err}",
                root.display()
            )
        })?;
        if !dir_entry
            .file_type()
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let path = dir_entry.path();
        let contents = match fs::read(&path) {
            Ok(contents) => contents,
            Err(_) => continue,
        };
        let entry: PolicyCacheEntry = match serde_json::from_slice(&contents) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.schema_version != POLICY_CACHE_SCHEMA_VERSION {
            continue;
        }
        snapshot.entries.push(BatchPolicyCacheEntryInfo {
            key: policy_cache_key(&entry.device_name, &entry.payload_signature),
            path: path.to_string_lossy().into_owned(),
            device_name: entry.device_name,
            payload_signature: entry.payload_signature,
            matches_per_batch: entry.matches_per_batch_cap,
            inflight_batches: entry.inflight_batches,
        });
    }
    snapshot.entries.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then(left.payload_signature.cmp(&right.payload_signature))
            .then(left.path.cmp(&right.path))
    });
    Ok(snapshot)
}

fn clear_policy_cache_entry_in_root(root: &Path, path: &Path) -> Result<bool, String> {
    if !path.starts_with(root) {
        return Err(format!(
            "refusing to delete Metal cache entry outside {}",
            root.display()
        ));
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!(
            "failed to delete Metal cache entry {}: {err}",
            path.display()
        )),
    }
}

fn clear_policy_cache_in_root(root: &Path) -> Result<usize, String> {
    let snapshot = snapshot_policy_cache_from_dir(root)?;
    let mut removed = 0usize;
    for entry in snapshot.entries {
        if clear_policy_cache_entry_in_root(root, Path::new(&entry.path))? {
            removed += 1;
        }
    }
    Ok(removed)
}

fn preferred_inflight_batches(device_name: &str) -> usize {
    if device_name.contains("Ultra") {
        5
    } else if device_name.contains("Max") {
        4
    } else if device_name.contains("Pro") {
        3
    } else {
        2
    }
}

fn preferred_base_limit(device_name: &str, payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) if device_name.contains("Max") => 262_144usize,
        BatchPayload::Fsm(_) => 131_072usize,
        BatchPayload::Ca(_) => 65_536usize,
        BatchPayload::Tm(_) => 32_768usize,
    }
}

fn candidate_batch_ceiling(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) => 262_144usize,
        BatchPayload::Ca(_) => 131_072usize,
        BatchPayload::Tm(_) => 65_536usize,
    }
}

fn benchmark_match_floor(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) => 131_072usize,
        BatchPayload::Ca(_) => 65_536usize,
        BatchPayload::Tm(_) => 32_768usize,
    }
}

fn benchmark_match_ceiling(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) => 1_048_576usize,
        BatchPayload::Ca(_) => 262_144usize,
        BatchPayload::Tm(_) => 131_072usize,
    }
}

fn candidate_policies(
    payload: &BatchPayload,
    device_name: &str,
    by_memory: usize,
) -> Vec<BatchExecutionPolicy> {
    let default_inflight = preferred_inflight_batches(device_name);
    let default_base = preferred_base_limit(device_name, payload);
    let candidate_max = candidate_batch_ceiling(payload).min(by_memory).max(4_096);
    let base = default_base.min(candidate_max).max(4_096);
    let mut batch_caps = vec![
        (base / 2).max(4_096),
        ((base.saturating_mul(3)) / 4).max(4_096),
        base,
        (base.saturating_mul(2)).min(candidate_max).max(4_096),
    ];
    batch_caps.sort_unstable();
    batch_caps.dedup();

    let mut inflight_candidates = vec![default_inflight];
    if default_inflight < 5 {
        inflight_candidates.push(default_inflight + 1);
    }

    let mut candidates = Vec::new();
    for inflight_batches in inflight_candidates {
        for matches_per_batch in &batch_caps {
            candidates.push(BatchExecutionPolicy {
                matches_per_batch: *matches_per_batch,
                inflight_batches,
            });
        }
    }
    candidates
}

fn benchmark_pairs(payload: &BatchPayload, candidates: &[BatchExecutionPolicy]) -> Vec<MatchPair> {
    let strategy_count = payload_strategy_count(payload).max(1);
    let max_batch = candidates
        .iter()
        .map(|candidate| candidate.matches_per_batch)
        .max()
        .unwrap_or(4_096);
    let max_inflight = candidates
        .iter()
        .map(|candidate| candidate.inflight_batches)
        .max()
        .unwrap_or(1);
    let pair_count = max_batch
        .saturating_mul(max_inflight)
        .saturating_mul(2)
        .clamp(
            benchmark_match_floor(payload),
            benchmark_match_ceiling(payload),
        );
    (0..pair_count)
        .map(|idx| {
            let a_idx = (idx % strategy_count) as u32;
            let b_idx = ((strategy_count - 1) - (idx % strategy_count)) as u32;
            MatchPair { a_idx, b_idx }
        })
        .collect()
}

fn benchmark_policy(
    prepared: &PreparedBatch,
    policy: BatchExecutionPolicy,
    pairs: &[MatchPair],
) -> Result<f64, String> {
    let start = Instant::now();
    let mut pending = Vec::new();
    for chunk in pairs.chunks(policy.matches_per_batch.max(1)) {
        let batch = try_begin_prepared_batch(prepared, chunk)?
            .ok_or_else(|| "Metal batch benchmark failed to begin dispatch".to_string())?;
        pending.push(batch);
        if pending.len() >= policy.inflight_batches.max(1) {
            let ready = pending.remove(0);
            let _ = try_finish_prepared_batch(ready)?;
        }
    }
    for ready in pending {
        let _ = try_finish_prepared_batch(ready)?;
    }
    Ok(start.elapsed().as_secs_f64())
}

pub fn recommended_batch_policy(
    config: &BatchEvalConfig,
    payload: &BatchPayload,
) -> Result<Option<RecommendedBatchPolicy>, String> {
    let key = ShaderKey::for_payload(payload);
    let ctx = context_for_key(key)?;
    let working_set = ctx.device.recommended_max_working_set_size();
    let device_name = ctx.device.name().to_string();
    let inflight_batches = preferred_inflight_batches(&device_name);
    let base_limit = preferred_base_limit(&device_name, payload);
    let target_dynamic_budget = if working_set > 0 {
        (working_set / 128).clamp(32 * 1024 * 1024, 256 * 1024 * 1024) as usize
    } else {
        64 * 1024 * 1024
    };
    let static_bytes = payload_static_bytes(payload);
    let available_dynamic =
        target_dynamic_budget.saturating_sub(static_bytes.min(target_dynamic_budget / 2));
    let bytes_per_pair = size_of::<MatchPairPod>() + size_of::<ScorePairPod>();
    let by_memory = available_dynamic
        .checked_div(bytes_per_pair.saturating_mul(inflight_batches))
        .unwrap_or(0)
        .max(4_096);
    let default_policy = BatchExecutionPolicy {
        matches_per_batch: base_limit.min(by_memory).max(4_096),
        inflight_batches,
    };
    let signature = payload_signature(payload);
    let cache_key = policy_cache_key(&device_name, &signature);
    let cache_path = policy_cache_root().map(|root| {
        policy_cache_path(&root, &device_name, &signature)
            .to_string_lossy()
            .into_owned()
    });
    if let Some(entry) = load_cached_policy(&device_name, &signature) {
        return Ok(Some(RecommendedBatchPolicy {
            policy: BatchExecutionPolicy {
                matches_per_batch: entry.matches_per_batch_cap.min(by_memory).max(4_096),
                inflight_batches: entry.inflight_batches.max(1),
            },
            source: BatchPolicySource::Cached,
            cache_key: Some(cache_key),
            cache_path,
        }));
    }

    let Some(prepared) = try_prepare_batch(config, payload)? else {
        return Ok(Some(RecommendedBatchPolicy {
            policy: default_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    };
    let candidates = candidate_policies(payload, &device_name, by_memory);
    if candidates.len() <= 1 {
        return Ok(Some(RecommendedBatchPolicy {
            policy: default_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    }
    let pairs = benchmark_pairs(payload, &candidates);
    if pairs.is_empty() {
        return Ok(Some(RecommendedBatchPolicy {
            policy: default_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    }
    let mut best_policy = default_policy;
    let mut best_elapsed = f64::INFINITY;
    for candidate in candidates {
        let elapsed = benchmark_policy(&prepared, candidate, &pairs)?;
        if elapsed < best_elapsed {
            best_elapsed = elapsed;
            best_policy = candidate;
        }
    }
    persist_cached_policy(&PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name,
        payload_signature: signature,
        matches_per_batch_cap: best_policy.matches_per_batch,
        inflight_batches: best_policy.inflight_batches,
    });
    Ok(Some(RecommendedBatchPolicy {
        policy: best_policy,
        source: BatchPolicySource::Benchmarked,
        cache_key: Some(cache_key),
        cache_path,
    }))
}

pub fn batch_policy_cache_snapshot() -> Result<BatchPolicyCacheSnapshot, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(BatchPolicyCacheSnapshot::default());
    };
    snapshot_policy_cache_from_dir(&root)
}

pub fn clear_batch_policy_cache_entry(path: &str) -> Result<bool, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(false);
    };
    clear_policy_cache_entry_in_root(&root, Path::new(path))
}

pub fn clear_batch_policy_cache() -> Result<usize, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(0);
    };
    clear_policy_cache_in_root(&root)
}

#[cfg(test)]
mod tests {
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
}

impl MetalContext {
    fn new_for_key(key: ShaderKey) -> Result<Self, String> {
        let device =
            Device::system_default().ok_or_else(|| "Metal device unavailable".to_string())?;
        let options = CompileOptions::new();
        let source = shader_source_for_key(key);
        let library = device.new_library_with_source(&source, &options)?;
        let fsm_fn = library
            .get_function("fsm_batch", None)
            .map_err(|err| err.to_string())?;
        let ca_fn = library
            .get_function("ca_batch", None)
            .map_err(|err| err.to_string())?;
        let tm_fn = library
            .get_function("tm_batch", None)
            .map_err(|err| err.to_string())?;
        let fsm_pipeline = device.new_compute_pipeline_state_with_function(&fsm_fn)?;
        let ca_pipeline = device.new_compute_pipeline_state_with_function(&ca_fn)?;
        let tm_pipeline = device.new_compute_pipeline_state_with_function(&tm_fn)?;
        let queue = device.new_command_queue();
        Ok(Self {
            device,
            queue,
            _library: library,
            fsm_pipeline,
            ca_pipeline,
            tm_pipeline,
        })
    }
}

fn context_for_key(key: ShaderKey) -> Result<&'static MetalContext, String> {
    static CONTEXTS: OnceLock<Mutex<HashMap<ShaderKey, Result<&'static MetalContext, String>>>> =
        OnceLock::new();
    let contexts = CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut contexts = contexts
        .lock()
        .map_err(|_| "Metal context cache lock poisoned".to_string())?;
    if let Some(existing) = contexts.get(&key) {
        return existing.clone();
    }
    let built = MetalContext::new_for_key(key);
    let stored = built.map(|ctx| Box::leak(Box::new(ctx)) as &'static MetalContext);
    contexts.insert(key, stored.clone());
    stored
}

fn eval_params(config: &BatchEvalConfig, pair_count: usize) -> EvalParams {
    EvalParams {
        rounds: config.rounds,
        pair_count: pair_count as u32,
        cc_a: config.payoff[0][0][0],
        cc_b: config.payoff[0][0][1],
        cd_a: config.payoff[0][1][0],
        cd_b: config.payoff[0][1][1],
        dc_a: config.payoff[1][0][0],
        dc_b: config.payoff[1][0][1],
        dd_a: config.payoff[1][1][0],
        dd_b: config.payoff[1][1][1],
        timeout_lose: config.timeout_lose,
        timeout_win: config.timeout_win,
    }
}

fn buffer_from_slice<T>(device: &Device, slice: &[T]) -> metal::Buffer {
    if slice.is_empty() {
        return device.new_buffer(1, MTLResourceOptions::StorageModeShared);
    }
    let len = size_of_val(slice) as u64;
    device.new_buffer_with_data(
        slice.as_ptr() as *const c_void,
        len,
        MTLResourceOptions::StorageModeShared,
    )
}

fn empty_output_buffer<T>(device: &Device, len: usize) -> metal::Buffer {
    device.new_buffer(
        (len.max(1) * size_of::<T>()) as u64,
        MTLResourceOptions::StorageModeShared,
    )
}

unsafe fn read_scores(buffer: &metal::BufferRef, len: usize) -> Vec<ScorePair> {
    let ptr = buffer.contents() as *const ScorePairPod;
    let slice = slice::from_raw_parts(ptr, len);
    slice
        .iter()
        .map(|score| ScorePair {
            a_total: score.a_total,
            b_total: score.b_total,
        })
        .collect()
}

unsafe fn read_tm_halting(buffer: &metal::BufferRef, len: usize) -> Vec<TmHaltingPair> {
    let ptr = buffer.contents() as *const TmHaltingPairPod;
    let slice = slice::from_raw_parts(ptr, len);
    slice
        .iter()
        .map(|entry| TmHaltingPair {
            a_all_halted: entry.a_all_halted != 0,
            b_all_halted: entry.b_all_halted != 0,
        })
        .collect()
}

fn submit_dispatch(
    pipeline: &ComputePipelineState,
    queue: &metal::CommandQueue,
    encode: impl FnOnce(&metal::ComputeCommandEncoderRef),
    pair_count: usize,
) -> metal::CommandBuffer {
    let command_buffer = queue.new_command_buffer();
    let encoder = command_buffer.new_compute_command_encoder();
    encoder.set_compute_pipeline_state(pipeline);
    encode(encoder);
    let width = pipeline.thread_execution_width().max(1);
    let threads_per_group = MTLSize {
        width,
        height: 1,
        depth: 1,
    };
    let group_count = MTLSize {
        width: (pair_count as u64).div_ceil(width),
        height: 1,
        depth: 1,
    };
    encoder.dispatch_thread_groups(group_count, threads_per_group);
    encoder.end_encoding();
    command_buffer.commit();
    command_buffer.to_owned()
}

fn pair_pods(pairs: &[MatchPair]) -> Vec<MatchPairPod> {
    pairs
        .iter()
        .map(|pair| MatchPairPod {
            a_idx: pair.a_idx,
            b_idx: pair.b_idx,
        })
        .collect()
}

fn tm_pods(transitions: &[TmTransitionPacked]) -> Vec<TmTransitionPod> {
    transitions
        .iter()
        .map(|trans| TmTransitionPod {
            write: trans.write,
            move_dir: trans.move_dir,
            next: trans.next,
            pad: 0,
        })
        .collect()
}

fn submit_prepared_batch_impl(
    ctx: &MetalContext,
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<PendingBatch, String> {
    let pair_pods = pair_pods(pairs);
    let pair_buffer = buffer_from_slice(&ctx.device, &pair_pods);
    let scores = empty_output_buffer::<ScorePairPod>(&ctx.device, pair_pods.len());
    let mut tm_halting_buffer = None;
    let eval = eval_params(&prepared.eval, pair_pods.len());
    let command_buffer = match &prepared.payload {
        PreparedPayload::Fsm {
            params,
            starts,
            outputs,
            transitions,
        } => submit_dispatch(
            &ctx.fsm_pipeline,
            &ctx.queue,
            |encoder| {
                encoder.set_buffer(0, Some(&pair_buffer), 0);
                encoder.set_buffer(1, Some(starts), 0);
                encoder.set_buffer(2, Some(outputs), 0);
                encoder.set_buffer(3, Some(transitions), 0);
                encoder.set_buffer(4, Some(&scores), 0);
                encoder.set_bytes(
                    5,
                    size_of::<EvalParams>() as u64,
                    (&eval as *const EvalParams).cast(),
                );
                encoder.set_bytes(
                    6,
                    size_of::<FsmParams>() as u64,
                    (params as *const FsmParams).cast(),
                );
            },
            pair_pods.len(),
        ),
        PreparedPayload::Ca {
            params,
            rule_tables,
        } => submit_dispatch(
            &ctx.ca_pipeline,
            &ctx.queue,
            |encoder| {
                encoder.set_buffer(0, Some(&pair_buffer), 0);
                encoder.set_buffer(1, Some(rule_tables), 0);
                encoder.set_buffer(2, Some(&scores), 0);
                encoder.set_bytes(
                    3,
                    size_of::<EvalParams>() as u64,
                    (&eval as *const EvalParams).cast(),
                );
                encoder.set_bytes(
                    4,
                    size_of::<CaParams>() as u64,
                    (params as *const CaParams).cast(),
                );
            },
            pair_pods.len(),
        ),
        PreparedPayload::Tm {
            params,
            starts,
            transitions,
        } => {
            let halting = empty_output_buffer::<TmHaltingPairPod>(&ctx.device, pair_pods.len());
            let command_buffer = submit_dispatch(
                &ctx.tm_pipeline,
                &ctx.queue,
                |encoder| {
                    encoder.set_buffer(0, Some(&pair_buffer), 0);
                    encoder.set_buffer(1, Some(starts), 0);
                    encoder.set_buffer(2, Some(transitions), 0);
                    encoder.set_buffer(3, Some(&scores), 0);
                    encoder.set_bytes(
                        4,
                        size_of::<EvalParams>() as u64,
                        (&eval as *const EvalParams).cast(),
                    );
                    encoder.set_bytes(
                        5,
                        size_of::<TmParams>() as u64,
                        (params as *const TmParams).cast(),
                    );
                    encoder.set_buffer(6, Some(&halting), 0);
                },
                pair_pods.len(),
            );
            tm_halting_buffer = Some(halting);
            command_buffer
        }
    };
    Ok(PendingBatch {
        _pair_buffer: pair_buffer,
        score_buffer: scores,
        tm_halting_buffer,
        command_buffer,
        pair_count: pair_pods.len(),
    })
}

pub fn try_evaluate_batch(request: &BatchRequest) -> Result<Option<Vec<ScorePair>>, String> {
    let eval = BatchEvalConfig {
        rounds: request.common.rounds,
        payoff: request.common.payoff,
        timeout_lose: request.common.timeout_lose,
        timeout_win: request.common.timeout_win,
    };
    let Some(prepared) = try_prepare_batch(&eval, &request.payload)? else {
        return Ok(None);
    };
    try_evaluate_prepared_batch(&prepared, &request.common.pairs)
}

pub fn try_prepare_batch(
    config: &BatchEvalConfig,
    payload: &BatchPayload,
) -> Result<Option<PreparedBatch>, String> {
    let shader_key = ShaderKey::for_payload(payload);
    let ctx = context_for_key(shader_key)?;

    let payload = match payload {
        BatchPayload::Fsm(payload) => PreparedPayload::Fsm {
            params: FsmParams {
                states: payload.states,
                alphabet: payload.alphabet,
            },
            starts: buffer_from_slice(&ctx.device, &payload.starts),
            outputs: buffer_from_slice(&ctx.device, &payload.outputs),
            transitions: buffer_from_slice(&ctx.device, &payload.transitions),
        },
        BatchPayload::Ca(payload) => PreparedPayload::Ca {
            params: CaParams {
                symbols: payload.symbols,
                two_r: payload.two_r,
                steps: payload.steps,
                rule_table_len: payload.rule_table_len,
            },
            rule_tables: buffer_from_slice(&ctx.device, &payload.rule_tables),
        },
        BatchPayload::Tm(payload) => {
            let transitions = tm_pods(&payload.transitions);
            PreparedPayload::Tm {
                params: TmParams {
                    states: payload.states,
                    symbols: payload.symbols,
                    blank: payload.blank,
                    max_steps: payload.max_steps,
                    transitions_per_strategy: payload.states.saturating_mul(payload.symbols),
                },
                starts: buffer_from_slice(&ctx.device, &payload.start_states),
                transitions: buffer_from_slice(&ctx.device, &transitions),
            }
        }
    };
    Ok(Some(PreparedBatch {
        shader_key,
        eval: config.clone(),
        payload,
    }))
}

pub fn try_evaluate_prepared_batch(
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<Option<Vec<ScorePair>>, String> {
    if pairs.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(pending) = try_begin_prepared_batch(prepared, pairs)? else {
        return Ok(None);
    };
    try_finish_prepared_batch(pending).map(Some)
}

pub fn try_evaluate_prepared_tm_halting_batch(
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<Option<Vec<TmHaltingPair>>, String> {
    if pairs.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(pending) = try_begin_prepared_batch(prepared, pairs)? else {
        return Ok(None);
    };
    try_finish_prepared_tm_halting_batch(pending).map(Some)
}

pub fn try_begin_prepared_batch(
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<Option<PendingBatch>, String> {
    if pairs.is_empty() {
        return Ok(None);
    }
    let ctx = context_for_key(prepared.shader_key)?;
    submit_prepared_batch_impl(ctx, prepared, pairs).map(Some)
}

pub fn try_finish_prepared_batch(pending: PendingBatch) -> Result<Vec<ScorePair>, String> {
    pending.command_buffer.wait_until_completed();
    Ok(unsafe { read_scores(&pending.score_buffer, pending.pair_count) })
}

pub fn try_finish_prepared_tm_halting_batch(
    pending: PendingBatch,
) -> Result<Vec<TmHaltingPair>, String> {
    pending.command_buffer.wait_until_completed();
    let Some(buffer) = pending.tm_halting_buffer.as_ref() else {
        return Err("TM halting results are only available for TM prepared batches".into());
    };
    Ok(unsafe { read_tm_halting(buffer, pending.pair_count) })
}

pub fn prewarm_default_batch_shaders() -> Result<(), String> {
    let _ = context_for_key(ShaderKey::defaults())?;
    Ok(())
}
