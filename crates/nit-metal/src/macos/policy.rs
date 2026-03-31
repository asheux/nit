//! Batch execution policy: heuristics, caching, and GPU benchmarking.
//!
//! Determines optimal batch sizes and inflight counts based on device
//! capabilities, payload characteristics, and empirical benchmarks.

use crate::{
    BatchEvalConfig, BatchExecutionPolicy, BatchPayload, BatchPolicyCacheEntryInfo,
    BatchPolicyCacheSnapshot, BatchPolicySource, MatchPair, RecommendedBatchPolicy,
    TmTransitionPacked,
};
use nit_utils::{fs::write_atomic, hashing::stable_hash_bytes, paths::cache_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::dispatch::{
    bytes_per_match_pair, try_begin_prepared_batch, try_finish_prepared_batch, try_prepare_batch,
    PendingBatch, PreparedBatch,
};
use super::shader::{context_for_key, ShaderKey};

pub(crate) const POLICY_CACHE_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Policy cache entry (persisted to disk as JSON)
// ---------------------------------------------------------------------------

/// On-disk representation of a benchmarked batch policy result.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PolicyCacheEntry {
    pub(crate) schema_version: u32,
    pub(crate) device_name: String,
    pub(crate) payload_signature: String,
    pub(crate) matches_per_batch_cap: usize,
    pub(crate) inflight_batches: usize,
}

// ---------------------------------------------------------------------------
// Payload introspection
// ---------------------------------------------------------------------------

/// Total GPU buffer bytes for the static (non-per-pair) payload data.
fn payload_static_bytes(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(fsm) => {
            fsm.starts.len() * size_of::<u32>()
                + fsm.outputs.len() * size_of::<u32>()
                + fsm.transitions.len() * size_of::<u32>()
        }
        BatchPayload::Ca(ca) => ca.rule_tables.len() * size_of::<u32>(),
        BatchPayload::Tm(tm) => {
            tm.start_states.len() * size_of::<u32>()
                + tm.transitions.len() * size_of::<TmTransitionPacked>()
        }
    }
}

/// Number of distinct strategies encoded in the payload.
fn payload_strategy_count(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(fsm) => fsm.starts.len(),
        BatchPayload::Ca(ca) => {
            let elements_per_strategy = ca.rule_table_len.max(1) as usize;
            ca.rule_tables.len() / elements_per_strategy
        }
        BatchPayload::Tm(tm) => tm.start_states.len(),
    }
}

/// Generates a human-readable signature for cache key derivation.
///
/// Captures the payload type, key dimensions, strategy count, and a
/// power-of-two bucket of the static buffer size in MiB.
pub(crate) fn payload_signature(payload: &BatchPayload) -> String {
    let static_mib_bucket = {
        let raw_bytes = payload_static_bytes(payload);
        let mib_rounded = raw_bytes.div_ceil(1024 * 1024).max(1);
        mib_rounded.next_power_of_two()
    };

    match payload {
        BatchPayload::Fsm(fsm) => format!(
            "fsm_s{}_a{}_n{}_static{}mib",
            fsm.states,
            fsm.alphabet,
            fsm.starts.len(),
            static_mib_bucket
        ),
        BatchPayload::Ca(ca) => format!(
            "ca_sym{}_twor{}_steps{}_table{}_n{}_static{}mib",
            ca.symbols,
            ca.two_r,
            ca.steps,
            ca.rule_table_len,
            payload_strategy_count(&BatchPayload::Ca(ca.clone())),
            static_mib_bucket
        ),
        BatchPayload::Tm(tm) => format!(
            "tm_s{}_sym{}_steps{}_n{}_static{}mib",
            tm.states,
            tm.symbols,
            tm.max_steps,
            tm.start_states.len(),
            static_mib_bucket
        ),
    }
}

// ---------------------------------------------------------------------------
// Cache path helpers
// ---------------------------------------------------------------------------

/// Replaces non-alphanumeric characters with underscores for filesystem safety.
fn sanitize_cache_component(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut after_separator = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            cleaned.push(ch.to_ascii_lowercase());
            after_separator = false;
            continue;
        }
        if !after_separator {
            cleaned.push('_');
            after_separator = true;
        }
    }

    cleaned.trim_matches('_').to_string()
}

/// Root directory for Metal policy cache files.
fn policy_cache_root() -> Option<PathBuf> {
    cache_dir().map(|base| base.join("games").join("metal-policy"))
}

/// Deterministic cache key combining device name and payload signature.
fn policy_cache_key(device_name: &str, sig: &str) -> String {
    let device_slug = sanitize_cache_component(device_name);
    let content_hash = stable_hash_bytes(format!("{device_name}:{sig}").as_bytes());
    format!("{device_slug}_{content_hash}")
}

/// Full filesystem path for a cache entry.
fn policy_cache_path(root: &Path, device_name: &str, sig: &str) -> PathBuf {
    root.join(format!(
        "{}_v{}.json",
        policy_cache_key(device_name, sig),
        POLICY_CACHE_SCHEMA_VERSION
    ))
}

// ---------------------------------------------------------------------------
// Cache CRUD operations
// ---------------------------------------------------------------------------

/// Loads a cached policy entry if it passes validation checks.
pub(crate) fn load_cached_policy_from_dir(
    root: &Path,
    device_name: &str,
    sig: &str,
) -> Option<PolicyCacheEntry> {
    let cache_file = policy_cache_path(root, device_name, sig);
    let raw_json = fs::read(cache_file).ok()?;
    let entry: PolicyCacheEntry = serde_json::from_slice(&raw_json).ok()?;

    let version_matches = entry.schema_version == POLICY_CACHE_SCHEMA_VERSION;
    let device_matches = entry.device_name == device_name;
    let signature_matches = entry.payload_signature == sig;
    let values_valid = entry.matches_per_batch_cap > 0 && entry.inflight_batches > 0;

    if version_matches && device_matches && signature_matches && values_valid {
        Some(entry)
    } else {
        None
    }
}

/// Loads a cached policy using the default cache root.
fn load_cached_policy(device_name: &str, sig: &str) -> Option<PolicyCacheEntry> {
    let root = policy_cache_root()?;
    load_cached_policy_from_dir(&root, device_name, sig)
}

/// Atomically writes a policy entry to disk.
pub(crate) fn persist_cached_policy_from_dir(root: &Path, entry: &PolicyCacheEntry) {
    if fs::create_dir_all(root).is_err() {
        return;
    }
    let target = policy_cache_path(root, &entry.device_name, &entry.payload_signature);
    let _ = write_atomic(&target, |writer| {
        serde_json::to_writer(writer, entry).map_err(std::io::Error::other)
    });
}

/// Persists a policy entry using the default cache root.
fn persist_cached_policy(entry: &PolicyCacheEntry) {
    let Some(root) = policy_cache_root() else {
        return;
    };
    persist_cached_policy_from_dir(&root, entry);
}

/// Reads all valid cache entries from a directory into a snapshot.
pub(crate) fn snapshot_policy_cache_from_dir(
    root: &Path,
) -> Result<BatchPolicyCacheSnapshot, String> {
    let mut snapshot = BatchPolicyCacheSnapshot {
        root: Some(root.to_string_lossy().into_owned()),
        entries: Vec::new(),
    };

    let dir_listing = match fs::read_dir(root) {
        Ok(listing) => listing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(snapshot),
        Err(err) => {
            return Err(format!(
                "failed to read Metal policy cache {}: {err}",
                root.display()
            ));
        }
    };

    for dir_result in dir_listing {
        let dir_entry = dir_result.map_err(|err| {
            format!(
                "failed to enumerate Metal policy cache {}: {err}",
                root.display()
            )
        })?;

        let is_regular_file = dir_entry
            .file_type()
            .map(|ft| ft.is_file())
            .unwrap_or(false);
        if !is_regular_file {
            continue;
        }

        let entry_path = dir_entry.path();
        let Ok(raw_json) = fs::read(&entry_path) else {
            continue;
        };
        let Ok(parsed): Result<PolicyCacheEntry, _> = serde_json::from_slice(&raw_json) else {
            continue;
        };
        if parsed.schema_version != POLICY_CACHE_SCHEMA_VERSION {
            continue;
        }

        snapshot.entries.push(BatchPolicyCacheEntryInfo {
            key: policy_cache_key(&parsed.device_name, &parsed.payload_signature),
            path: entry_path.to_string_lossy().into_owned(),
            device_name: parsed.device_name,
            payload_signature: parsed.payload_signature,
            matches_per_batch: parsed.matches_per_batch_cap,
            inflight_batches: parsed.inflight_batches,
        });
    }

    snapshot.entries.sort_by(|lhs, rhs| {
        lhs.key
            .cmp(&rhs.key)
            .then(lhs.payload_signature.cmp(&rhs.payload_signature))
            .then(lhs.path.cmp(&rhs.path))
    });

    Ok(snapshot)
}

/// Deletes a single cache entry, validating it lives under the root.
pub(crate) fn clear_policy_cache_entry_in_root(
    root: &Path,
    target: &Path,
) -> Result<bool, String> {
    if !target.starts_with(root) {
        return Err(format!(
            "refusing to delete Metal cache entry outside {}",
            root.display()
        ));
    }
    match fs::remove_file(target) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!(
            "failed to delete Metal cache entry {}: {err}",
            target.display()
        )),
    }
}

/// Removes all cached policy entries under a root directory.
pub(crate) fn clear_policy_cache_in_root(root: &Path) -> Result<usize, String> {
    let current = snapshot_policy_cache_from_dir(root)?;
    let mut removed_count = 0usize;
    for entry in current.entries {
        if clear_policy_cache_entry_in_root(root, Path::new(&entry.path))? {
            removed_count += 1;
        }
    }
    Ok(removed_count)
}

// ---------------------------------------------------------------------------
// Device-aware heuristics
// ---------------------------------------------------------------------------

/// Returns the recommended inflight batch count for a given Apple Silicon tier.
pub(crate) fn preferred_inflight_batches(device_name: &str) -> usize {
    if device_name.contains("Ultra") {
        return 5;
    }
    if device_name.contains("Max") {
        return 4;
    }
    if device_name.contains("Pro") {
        return 3;
    }
    2
}

/// Returns the base matches-per-batch limit by device tier and payload type.
pub(crate) fn preferred_base_limit(device_name: &str, payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) if device_name.contains("Max") => 262_144,
        BatchPayload::Fsm(_) => 131_072,
        BatchPayload::Ca(_) => 65_536,
        BatchPayload::Tm(_) => 32_768,
    }
}

/// Upper bound on batch size for policy candidate generation.
fn candidate_batch_ceiling(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) => 262_144,
        BatchPayload::Ca(_) => 131_072,
        BatchPayload::Tm(_) => 65_536,
    }
}

/// Minimum pairs for a meaningful benchmark run.
fn benchmark_pair_floor(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) => 131_072,
        BatchPayload::Ca(_) => 65_536,
        BatchPayload::Tm(_) => 32_768,
    }
}

/// Maximum pairs for benchmark runs to cap memory usage.
fn benchmark_pair_ceiling(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) => 1_048_576,
        BatchPayload::Ca(_) => 262_144,
        BatchPayload::Tm(_) => 131_072,
    }
}

// ---------------------------------------------------------------------------
// Benchmark candidate generation
// ---------------------------------------------------------------------------

/// Generates candidate policies by varying batch size and inflight count.
fn candidate_policies(
    payload: &BatchPayload,
    device_name: &str,
    memory_cap: usize,
) -> Vec<BatchExecutionPolicy> {
    let default_inflight = preferred_inflight_batches(device_name);
    let base_batch = preferred_base_limit(device_name, payload);
    let max_batch = candidate_batch_ceiling(payload).min(memory_cap).max(4_096);
    let clamped_base = base_batch.min(max_batch).max(4_096);

    let mut batch_sizes = vec![
        (clamped_base / 2).max(4_096),
        ((clamped_base.saturating_mul(3)) / 4).max(4_096),
        clamped_base,
        (clamped_base.saturating_mul(2)).min(max_batch).max(4_096),
    ];
    batch_sizes.sort_unstable();
    batch_sizes.dedup();

    let mut inflight_options = vec![default_inflight];
    if default_inflight < 5 {
        inflight_options.push(default_inflight + 1);
    }

    inflight_options
        .iter()
        .flat_map(|&inflight| {
            batch_sizes.iter().map(move |&batch_size| {
                BatchExecutionPolicy {
                    matches_per_batch: batch_size,
                    inflight_batches: inflight,
                }
            })
        })
        .collect()
}

/// Creates a set of synthetic match pairs for benchmarking GPU throughput.
fn generate_benchmark_pairs(
    payload: &BatchPayload,
    candidates: &[BatchExecutionPolicy],
) -> Vec<MatchPair> {
    let strategy_count = payload_strategy_count(payload).max(1);

    let largest_batch = candidates
        .iter()
        .map(|c| c.matches_per_batch)
        .max()
        .unwrap_or(4_096);
    let most_inflight = candidates
        .iter()
        .map(|c| c.inflight_batches)
        .max()
        .unwrap_or(1);

    let total_pairs = largest_batch
        .saturating_mul(most_inflight)
        .saturating_mul(2)
        .clamp(
            benchmark_pair_floor(payload),
            benchmark_pair_ceiling(payload),
        );

    (0..total_pairs)
        .map(|idx| {
            let forward_idx = (idx % strategy_count) as u32;
            let reverse_idx = ((strategy_count - 1) - (idx % strategy_count)) as u32;
            MatchPair {
                a_idx: forward_idx,
                b_idx: reverse_idx,
            }
        })
        .collect()
}

/// Runs a single benchmark trial for a candidate policy, returning elapsed seconds.
fn time_benchmark_trial(
    prepared: &PreparedBatch,
    candidate: BatchExecutionPolicy,
    all_pairs: &[MatchPair],
) -> Result<f64, String> {
    let chunk_size = candidate.matches_per_batch.max(1);
    let max_inflight = candidate.inflight_batches.max(1);
    let timer = Instant::now();

    let mut in_flight: Vec<PendingBatch> = Vec::new();
    for chunk in all_pairs.chunks(chunk_size) {
        let submitted = try_begin_prepared_batch(prepared, chunk)?
            .ok_or_else(|| "Metal batch benchmark failed to begin dispatch".to_string())?;
        in_flight.push(submitted);

        if in_flight.len() >= max_inflight {
            let oldest = in_flight.remove(0);
            let _ = try_finish_prepared_batch(oldest)?;
        }
    }
    for remaining in in_flight {
        let _ = try_finish_prepared_batch(remaining)?;
    }

    Ok(timer.elapsed().as_secs_f64())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Determines the best batch execution policy for a payload.
///
/// Checks the on-disk cache first, then falls back to GPU benchmarking
/// if no cached result exists. Returns a heuristic default if benchmarking
/// cannot be performed.
pub fn recommended_batch_policy(
    config: &BatchEvalConfig,
    payload: &BatchPayload,
) -> Result<Option<RecommendedBatchPolicy>, String> {
    let key = ShaderKey::for_payload(payload);
    let ctx = context_for_key(key)?;
    let device_name = ctx.device.name().to_string();
    let working_set = ctx.device.recommended_max_working_set_size();

    let inflight = preferred_inflight_batches(&device_name);
    let base_limit = preferred_base_limit(&device_name, payload);

    let dynamic_budget = if working_set > 0 {
        (working_set / 128).clamp(32 * 1024 * 1024, 256 * 1024 * 1024) as usize
    } else {
        64 * 1024 * 1024
    };

    let static_bytes = payload_static_bytes(payload);
    let available = dynamic_budget.saturating_sub(static_bytes.min(dynamic_budget / 2));

    let memory_cap = available
        .checked_div(bytes_per_match_pair().saturating_mul(inflight))
        .unwrap_or(0)
        .max(4_096);

    let heuristic_policy = BatchExecutionPolicy {
        matches_per_batch: base_limit.min(memory_cap).max(4_096),
        inflight_batches: inflight,
    };

    let signature = payload_signature(payload);
    let cache_key = policy_cache_key(&device_name, &signature);
    let cache_file_path = policy_cache_root().map(|root| {
        policy_cache_path(&root, &device_name, &signature)
            .to_string_lossy()
            .into_owned()
    });

    // Fast path: use a previously benchmarked result
    if let Some(cached) = load_cached_policy(&device_name, &signature) {
        return Ok(Some(RecommendedBatchPolicy {
            policy: BatchExecutionPolicy {
                matches_per_batch: cached.matches_per_batch_cap.min(memory_cap).max(4_096),
                inflight_batches: cached.inflight_batches.max(1),
            },
            source: BatchPolicySource::Cached,
            cache_key: Some(cache_key),
            cache_path: cache_file_path,
        }));
    }

    // Prepare a batch for benchmarking
    let Some(prepared) = try_prepare_batch(config, payload)? else {
        return Ok(Some(RecommendedBatchPolicy {
            policy: heuristic_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    };

    let candidates = candidate_policies(payload, &device_name, memory_cap);
    if candidates.len() <= 1 {
        return Ok(Some(RecommendedBatchPolicy {
            policy: heuristic_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    }

    let benchmark_pairs = generate_benchmark_pairs(payload, &candidates);
    if benchmark_pairs.is_empty() {
        return Ok(Some(RecommendedBatchPolicy {
            policy: heuristic_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    }

    // Run benchmarks across all candidates, pick the fastest
    let mut fastest_policy = heuristic_policy;
    let mut fastest_elapsed = f64::INFINITY;
    for candidate in candidates {
        let elapsed = time_benchmark_trial(&prepared, candidate, &benchmark_pairs)?;
        if elapsed < fastest_elapsed {
            fastest_elapsed = elapsed;
            fastest_policy = candidate;
        }
    }

    persist_cached_policy(&PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name,
        payload_signature: signature,
        matches_per_batch_cap: fastest_policy.matches_per_batch,
        inflight_batches: fastest_policy.inflight_batches,
    });

    Ok(Some(RecommendedBatchPolicy {
        policy: fastest_policy,
        source: BatchPolicySource::Benchmarked,
        cache_key: Some(cache_key),
        cache_path: cache_file_path,
    }))
}

/// Returns a snapshot of all cached batch policies.
pub fn batch_policy_cache_snapshot() -> Result<BatchPolicyCacheSnapshot, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(BatchPolicyCacheSnapshot::default());
    };
    snapshot_policy_cache_from_dir(&root)
}

/// Deletes a single cached policy entry by path.
pub fn clear_batch_policy_cache_entry(entry_path: &str) -> Result<bool, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(false);
    };
    clear_policy_cache_entry_in_root(&root, Path::new(entry_path))
}

/// Clears all cached policy entries.
pub fn clear_batch_policy_cache() -> Result<usize, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(0);
    };
    clear_policy_cache_in_root(&root)
}
