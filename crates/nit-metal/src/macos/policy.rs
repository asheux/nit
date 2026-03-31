//! Batch execution policy: heuristics and GPU benchmarking.
//!
//! Determines optimal batch sizes and inflight counts based on device
//! capabilities, payload characteristics, and empirical benchmarks.

use crate::{
    BatchEvalConfig, BatchExecutionPolicy, BatchPayload, BatchPolicySource, MatchPair,
    RecommendedBatchPolicy, TmTransitionPacked,
};
use std::mem::size_of;
use std::time::Instant;

use super::cache::{
    load_cached_policy, persist_cached_policy, policy_cache_key, policy_cache_path,
    policy_cache_root, PolicyCacheEntry, POLICY_CACHE_SCHEMA_VERSION,
};
use super::dispatch::{
    bytes_per_match_pair, try_begin_prepared_batch, try_finish_prepared_batch, try_prepare_batch,
    PendingBatch, PreparedBatch,
};
use super::shader::{context_for_key, ShaderKey};

/// Minimum batch size enforced across all policies and candidates.
const MIN_BATCH_SIZE: usize = 4_096;

/// Default memory budget (bytes) when the device working set is unavailable.
const DEFAULT_MEMORY_BUDGET: usize = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Payload introspection
// ---------------------------------------------------------------------------

/// Total GPU buffer bytes for the static (non-per-pair) payload data.
///
/// Accounts for strategy tables, transition tables, and rule tables
/// that are uploaded once regardless of how many match pairs are dispatched.
fn payload_static_bytes(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(fsm) => {
            let start_table_bytes = fsm.starts.len() * size_of::<u32>();
            let output_table_bytes = fsm.outputs.len() * size_of::<u32>();
            let transition_table_bytes = fsm.transitions.len() * size_of::<u32>();
            start_table_bytes + output_table_bytes + transition_table_bytes
        }
        BatchPayload::Ca(automaton) => automaton.rule_tables.len() * size_of::<u32>(),
        BatchPayload::Tm(machine) => {
            let start_table_bytes = machine.start_states.len() * size_of::<u32>();
            let transition_bytes = machine.transitions.len() * size_of::<TmTransitionPacked>();
            start_table_bytes + transition_bytes
        }
    }
}

/// Number of distinct strategies encoded in the payload.
///
/// For FSM and TM payloads this equals the start-state count;
/// for CA payloads it derives from the rule table length.
fn payload_strategy_count(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(fsm) => fsm.starts.len(),
        BatchPayload::Ca(automaton) => {
            let elements_per_strategy = automaton.rule_table_len.max(1) as usize;
            automaton.rule_tables.len() / elements_per_strategy
        }
        BatchPayload::Tm(machine) => machine.start_states.len(),
    }
}

/// Generates a human-readable signature for cache key derivation.
///
/// Captures the payload type, key dimensions, strategy count, and a
/// power-of-two bucket of the static buffer size in MiB.
pub(crate) fn payload_signature(payload: &BatchPayload) -> String {
    let raw_static_bytes = payload_static_bytes(payload);
    let mib_rounded = raw_static_bytes.div_ceil(1024 * 1024).max(1);
    let static_mib_bucket = mib_rounded.next_power_of_two();

    match payload {
        BatchPayload::Fsm(fsm) => format!(
            "fsm_s{}_a{}_n{}_static{}mib",
            fsm.states,
            fsm.alphabet,
            fsm.starts.len(),
            static_mib_bucket
        ),
        BatchPayload::Ca(automaton) => {
            let entries_per_strategy = automaton.rule_table_len.max(1) as usize;
            let strategy_total = automaton.rule_tables.len() / entries_per_strategy;
            format!(
                "ca_sym{}_twor{}_steps{}_table{}_n{}_static{}mib",
                automaton.symbols,
                automaton.two_r,
                automaton.steps,
                automaton.rule_table_len,
                strategy_total,
                static_mib_bucket
            )
        }
        BatchPayload::Tm(machine) => format!(
            "tm_s{}_sym{}_steps{}_n{}_static{}mib",
            machine.states,
            machine.symbols,
            machine.max_steps,
            machine.start_states.len(),
            static_mib_bucket
        ),
    }
}

// ---------------------------------------------------------------------------
// Device-aware heuristics
// ---------------------------------------------------------------------------

/// Returns the recommended inflight batch count for a given Apple Silicon tier.
///
/// Higher-end chips with more GPU cores benefit from deeper dispatch queues.
pub(crate) fn preferred_inflight_batches(gpu_device_name: &str) -> usize {
    if gpu_device_name.contains("Ultra") {
        return 5;
    }
    if gpu_device_name.contains("Max") {
        return 4;
    }
    if gpu_device_name.contains("Pro") {
        return 3;
    }
    2
}

/// Returns the base matches-per-batch limit by device tier and payload type.
///
/// FSM kernels are lightweight per-pair, so they benefit from larger batches.
/// TM kernels are heavier, requiring smaller batches to stay within budget.
pub(crate) fn preferred_base_limit(gpu_device_name: &str, payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(_) if gpu_device_name.contains("Max") => 262_144,
        BatchPayload::Fsm(_) => 131_072,
        BatchPayload::Ca(_) => 65_536,
        BatchPayload::Tm(_) => 32_768,
    }
}

// ---------------------------------------------------------------------------
// Batch size limits per payload type
// ---------------------------------------------------------------------------

/// Per-payload-type batch size limits for benchmarking and candidate generation.
///
/// Consolidates the ceiling and floor constants that govern benchmark scope
/// and policy candidate space, varying by computational weight per pair.
struct PayloadBatchLimits {
    /// Maximum batch size for policy candidate generation.
    candidate_ceiling: usize,

    /// Minimum pairs for a meaningful benchmark run.
    benchmark_floor: usize,

    /// Maximum pairs for benchmark runs to cap memory usage.
    benchmark_ceiling: usize,
}

impl PayloadBatchLimits {
    /// Derive limits from the payload type's computational characteristics.
    ///
    /// Lightweight payloads (FSM) get higher ceilings; heavy payloads (TM)
    /// use conservative limits to avoid excessive memory allocation.
    fn for_payload(payload: &BatchPayload) -> Self {
        match payload {
            BatchPayload::Fsm(_) => Self {
                candidate_ceiling: 262_144,
                benchmark_floor: 131_072,
                benchmark_ceiling: 1_048_576,
            },
            BatchPayload::Ca(_) => Self {
                candidate_ceiling: 131_072,
                benchmark_floor: 65_536,
                benchmark_ceiling: 262_144,
            },
            BatchPayload::Tm(_) => Self {
                candidate_ceiling: 65_536,
                benchmark_floor: 32_768,
                benchmark_ceiling: 131_072,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Benchmark candidate generation
// ---------------------------------------------------------------------------

/// Generates candidate policies by varying batch size and inflight count.
///
/// Produces a combinatorial grid of batch sizes (fractions and multiples
/// of the base limit) crossed with inflight depths near the device default.
fn candidate_policies(
    payload: &BatchPayload,
    gpu_device_name: &str,
    memory_cap: usize,
) -> Vec<BatchExecutionPolicy> {
    let default_inflight_depth = preferred_inflight_batches(gpu_device_name);
    let base_batch_count = preferred_base_limit(gpu_device_name, payload);
    let limits = PayloadBatchLimits::for_payload(payload);
    let max_viable_batch = limits.candidate_ceiling.min(memory_cap).max(MIN_BATCH_SIZE);
    let clamped_base = base_batch_count.min(max_viable_batch).max(MIN_BATCH_SIZE);

    let mut batch_size_candidates = vec![
        (clamped_base / 2).max(MIN_BATCH_SIZE),
        ((clamped_base.saturating_mul(3)) / 4).max(MIN_BATCH_SIZE),
        clamped_base,
        (clamped_base.saturating_mul(2))
            .min(max_viable_batch)
            .max(MIN_BATCH_SIZE),
    ];
    batch_size_candidates.sort_unstable();
    batch_size_candidates.dedup();

    let mut inflight_depth_options = vec![default_inflight_depth];
    if default_inflight_depth < 5 {
        inflight_depth_options.push(default_inflight_depth + 1);
    }

    inflight_depth_options
        .iter()
        .flat_map(|&depth| {
            batch_size_candidates.iter().map(move |&batch_count| {
                BatchExecutionPolicy {
                    matches_per_batch: batch_count,
                    inflight_batches: depth,
                }
            })
        })
        .collect()
}

/// Creates synthetic match pairs for benchmarking GPU throughput.
///
/// Pairs cycle through strategy indices to exercise the full strategy space
/// and ensure even GPU workload distribution across all compute units.
fn generate_benchmark_pairs(
    payload: &BatchPayload,
    candidates: &[BatchExecutionPolicy],
) -> Vec<MatchPair> {
    let total_strategies = payload_strategy_count(payload).max(1);
    let limits = PayloadBatchLimits::for_payload(payload);

    let largest_candidate_batch = candidates
        .iter()
        .map(|policy| policy.matches_per_batch)
        .max()
        .unwrap_or(MIN_BATCH_SIZE);

    let deepest_inflight_level = candidates
        .iter()
        .map(|policy| policy.inflight_batches)
        .max()
        .unwrap_or(1);

    let required_pair_count = largest_candidate_batch
        .saturating_mul(deepest_inflight_level)
        .saturating_mul(2)
        .clamp(limits.benchmark_floor, limits.benchmark_ceiling);

    (0..required_pair_count)
        .map(|pair_index| {
            let forward_strategy = (pair_index % total_strategies) as u32;
            let reverse_strategy =
                ((total_strategies - 1) - (pair_index % total_strategies)) as u32;
            MatchPair {
                a_idx: forward_strategy,
                b_idx: reverse_strategy,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Benchmark execution
// ---------------------------------------------------------------------------

/// Waits for all remaining pending GPU batches to complete.
///
/// Called after the main dispatch loop to flush the in-flight queue.
fn drain_pending_batches(pending: Vec<PendingBatch>) -> Result<(), String> {
    for completed_batch in pending {
        let _ = try_finish_prepared_batch(completed_batch)?;
    }
    Ok(())
}

/// Runs a single benchmark trial for a candidate policy, returning elapsed seconds.
///
/// Simulates the real dispatch pattern: submitting batches in chunks and
/// draining the oldest completed batch when the inflight limit is reached.
fn time_benchmark_trial(
    prepared: &PreparedBatch,
    candidate: BatchExecutionPolicy,
    all_pairs: &[MatchPair],
) -> Result<f64, String> {
    let chunk_size = candidate.matches_per_batch.max(1);
    let inflight_limit = candidate.inflight_batches.max(1);
    let timer = Instant::now();

    let mut pending_queue: Vec<PendingBatch> = Vec::new();

    for pair_chunk in all_pairs.chunks(chunk_size) {
        let submitted_batch = try_begin_prepared_batch(prepared, pair_chunk)?
            .ok_or_else(|| "Metal batch benchmark failed to begin dispatch".to_string())?;
        pending_queue.push(submitted_batch);

        if pending_queue.len() >= inflight_limit {
            let oldest_batch = pending_queue.remove(0);
            let _ = try_finish_prepared_batch(oldest_batch)?;
        }
    }

    drain_pending_batches(pending_queue)?;
    Ok(timer.elapsed().as_secs_f64())
}

/// Runs benchmark trials across all candidates and returns the fastest policy.
///
/// Each candidate is timed over the same set of synthetic pairs. The policy
/// that finishes in the shortest wall-clock time wins.
fn select_fastest_policy(
    prepared: &PreparedBatch,
    candidates: Vec<BatchExecutionPolicy>,
    benchmark_pairs: &[MatchPair],
    fallback_policy: BatchExecutionPolicy,
) -> Result<BatchExecutionPolicy, String> {
    let mut best_policy = fallback_policy;
    let mut shortest_elapsed = f64::INFINITY;

    for candidate_policy in candidates {
        let trial_elapsed = time_benchmark_trial(prepared, candidate_policy, benchmark_pairs)?;
        if trial_elapsed < shortest_elapsed {
            shortest_elapsed = trial_elapsed;
            best_policy = candidate_policy;
        }
    }

    Ok(best_policy)
}

// ---------------------------------------------------------------------------
// Memory budget computation
// ---------------------------------------------------------------------------

/// Computes the maximum matches-per-batch given GPU memory constraints.
///
/// Derives a budget from the device's recommended working set, subtracts
/// static payload overhead, and divides by per-pair buffer requirements.
fn compute_memory_cap(
    working_set_size: u64,
    static_payload_bytes: usize,
    inflight_count: usize,
) -> usize {
    let dynamic_budget = if working_set_size > 0 {
        (working_set_size / 128).clamp(32 * 1024 * 1024, 256 * 1024 * 1024) as usize
    } else {
        DEFAULT_MEMORY_BUDGET
    };

    let reserved_for_payload = static_payload_bytes.min(dynamic_budget / 2);
    let available_for_pairs = dynamic_budget.saturating_sub(reserved_for_payload);

    available_for_pairs
        .checked_div(bytes_per_match_pair().saturating_mul(inflight_count))
        .unwrap_or(0)
        .max(MIN_BATCH_SIZE)
}

/// Resolves the cache key and optional file path for a device/signature pair.
///
/// Returns a tuple of `(cache_key, optional_file_path)` used for both
/// cache lookups and result metadata in [`RecommendedBatchPolicy`].
fn resolve_cache_location(
    gpu_device_name: &str,
    payload_sig: &str,
) -> (String, Option<String>) {
    let derived_key = policy_cache_key(gpu_device_name, payload_sig);

    let resolved_path = policy_cache_root().map(|root_dir| {
        policy_cache_path(&root_dir, gpu_device_name, payload_sig)
            .to_string_lossy()
            .into_owned()
    });

    (derived_key, resolved_path)
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
    let shader_key = ShaderKey::for_payload(payload);
    let metal_ctx = context_for_key(shader_key)?;
    let gpu_device_name = metal_ctx.device.name().to_string();

    let inflight_depth = preferred_inflight_batches(&gpu_device_name);
    let base_batch_limit = preferred_base_limit(&gpu_device_name, payload);
    let static_bytes = payload_static_bytes(payload);
    let memory_cap = compute_memory_cap(
        metal_ctx.device.recommended_max_working_set_size(),
        static_bytes,
        inflight_depth,
    );

    let heuristic_policy = BatchExecutionPolicy {
        matches_per_batch: base_batch_limit.min(memory_cap).max(MIN_BATCH_SIZE),
        inflight_batches: inflight_depth,
    };

    let payload_sig = payload_signature(payload);
    let (cache_key, cache_file_path) = resolve_cache_location(&gpu_device_name, &payload_sig);

    // Fast path: use a previously benchmarked result from disk.
    if let Some(cached_entry) = load_cached_policy(&gpu_device_name, &payload_sig) {
        return Ok(Some(RecommendedBatchPolicy {
            policy: BatchExecutionPolicy {
                matches_per_batch: cached_entry
                    .matches_per_batch_cap
                    .min(memory_cap)
                    .max(MIN_BATCH_SIZE),
                inflight_batches: cached_entry.inflight_batches.max(1),
            },
            source: BatchPolicySource::Cached,
            cache_key: Some(cache_key),
            cache_path: cache_file_path,
        }));
    }

    // Prepare a batch for GPU benchmarking.
    let Some(prepared_batch) = try_prepare_batch(config, payload)? else {
        return Ok(Some(RecommendedBatchPolicy {
            policy: heuristic_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    };

    let benchmark_candidates = candidate_policies(payload, &gpu_device_name, memory_cap);
    if benchmark_candidates.len() <= 1 {
        return Ok(Some(RecommendedBatchPolicy {
            policy: heuristic_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    }

    let synthetic_pairs = generate_benchmark_pairs(payload, &benchmark_candidates);
    if synthetic_pairs.is_empty() {
        return Ok(Some(RecommendedBatchPolicy {
            policy: heuristic_policy,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    }

    // Run the benchmark sweep and persist the winner.
    let fastest_policy = select_fastest_policy(
        &prepared_batch,
        benchmark_candidates,
        &synthetic_pairs,
        heuristic_policy,
    )?;

    persist_cached_policy(&PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name: gpu_device_name,
        payload_signature: payload_sig,
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
