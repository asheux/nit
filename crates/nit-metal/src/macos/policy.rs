//! Batch execution policy: heuristics and GPU benchmarking.
//!
//! Determines optimal batch sizes and inflight counts based on device
//! capabilities, payload characteristics, and empirical benchmarks.

use crate::{
    BatchEvalConfig, BatchExecutionPolicy, BatchPayload, BatchPolicySource, MatchPair,
    RecommendedBatchPolicy, TmTransitionPacked,
};
use std::collections::VecDeque;
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
    let u32_stride = size_of::<u32>();
    match payload {
        BatchPayload::Fsm(fsm) => {
            (fsm.starts.len() + fsm.outputs.len() + fsm.transitions.len()) * u32_stride
        }
        BatchPayload::Ca(automaton) => automaton.rule_tables.len() * u32_stride,
        BatchPayload::Tm(machine) => {
            let packed_stride = size_of::<TmTransitionPacked>();
            machine.start_states.len() * u32_stride
                + machine.transitions.len() * packed_stride
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

/// Generates candidate policies by varying batch size and inflight depth.
///
/// Produces a combinatorial grid: several fractions and multiples of the
/// device base limit, crossed with one or two inflight depths near the
/// device default. Duplicate sizes are collapsed.
fn candidate_policies(
    payload: &BatchPayload,
    device_name: &str,
    mem_cap: usize,
) -> Vec<BatchExecutionPolicy> {
    let base_depth = preferred_inflight_batches(device_name);
    let base_size = preferred_base_limit(device_name, payload);
    let limits = PayloadBatchLimits::for_payload(payload);
    let viable_ceil = limits.candidate_ceiling.min(mem_cap).max(MIN_BATCH_SIZE);
    let anchor = base_size.min(viable_ceil).max(MIN_BATCH_SIZE);

    let clamp = |v: usize| v.clamp(MIN_BATCH_SIZE, viable_ceil);
    let mut sizes = vec![
        clamp(anchor / 2),
        clamp(anchor.saturating_mul(3) / 4),
        anchor,
        clamp(anchor.saturating_mul(2)),
    ];
    sizes.sort_unstable();
    sizes.dedup();

    let depths: &[usize] = if base_depth < 5 {
        &[base_depth, base_depth + 1]
    } else {
        &[base_depth]
    };

    depths
        .iter()
        .flat_map(|&depth| {
            sizes.iter().map(move |&size| BatchExecutionPolicy {
                matches_per_batch: size,
                inflight_batches: depth,
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
    let strategy_count = payload_strategy_count(payload).max(1);
    let limits = PayloadBatchLimits::for_payload(payload);

    let max_batch = candidates
        .iter()
        .map(|p| p.matches_per_batch)
        .max()
        .unwrap_or(MIN_BATCH_SIZE);

    let max_depth = candidates
        .iter()
        .map(|p| p.inflight_batches)
        .max()
        .unwrap_or(1);

    let pair_count = max_batch
        .saturating_mul(max_depth)
        .saturating_mul(2)
        .clamp(limits.benchmark_floor, limits.benchmark_ceiling);

    (0..pair_count)
        .map(|i| {
            let fwd = (i % strategy_count) as u32;
            let rev = (strategy_count - 1 - i % strategy_count) as u32;
            MatchPair {
                a_idx: fwd,
                b_idx: rev,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Benchmark execution
// ---------------------------------------------------------------------------

/// Drains all remaining in-flight GPU batches, discarding their results.
///
/// Called after the main dispatch loop to flush the pipeline before
/// recording the elapsed wall-clock time.
fn drain_inflight(queue: VecDeque<PendingBatch>) -> Result<(), String> {
    for batch in queue {
        let _ = try_finish_prepared_batch(batch)?;
    }
    Ok(())
}

/// Runs a single benchmark trial for a candidate policy, returning elapsed seconds.
///
/// Simulates the real dispatch pattern: submitting batches in chunks and
/// retiring the oldest completed batch when the inflight depth is saturated.
/// Uses a `VecDeque` so that retiring the front is O(1).
fn time_benchmark_trial(
    prepared: &PreparedBatch,
    policy: BatchExecutionPolicy,
    pairs: &[MatchPair],
) -> Result<f64, String> {
    let chunk_size = policy.matches_per_batch.max(1);
    let depth_cap = policy.inflight_batches.max(1);
    let timer = Instant::now();

    let mut inflight: VecDeque<PendingBatch> = VecDeque::with_capacity(depth_cap + 1);

    for chunk in pairs.chunks(chunk_size) {
        let submitted = try_begin_prepared_batch(prepared, chunk)?
            .ok_or("Metal batch benchmark: dispatch failed to begin")?;
        inflight.push_back(submitted);

        if inflight.len() > depth_cap {
            let retired = inflight.pop_front().expect("inflight is non-empty");
            let _ = try_finish_prepared_batch(retired)?;
        }
    }

    drain_inflight(inflight)?;
    Ok(timer.elapsed().as_secs_f64())
}

/// Benchmarks every candidate policy and returns the one with the lowest
/// wall-clock time, falling back to `fallback` if the candidate set is empty.
fn select_fastest_policy(
    prepared: &PreparedBatch,
    candidates: Vec<BatchExecutionPolicy>,
    pairs: &[MatchPair],
    fallback: BatchExecutionPolicy,
) -> Result<BatchExecutionPolicy, String> {
    let timed: Vec<(BatchExecutionPolicy, f64)> = candidates
        .into_iter()
        .map(|policy| {
            let elapsed = time_benchmark_trial(prepared, policy, pairs)?;
            Ok((policy, elapsed))
        })
        .collect::<Result<_, String>>()?;

    let winner = timed
        .into_iter()
        .min_by(|(_, elapsed_a), (_, elapsed_b)| {
            elapsed_a.partial_cmp(elapsed_b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(policy, _)| policy)
        .unwrap_or(fallback);

    Ok(winner)
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
// Recommendation builders
// ---------------------------------------------------------------------------

/// Constructs a heuristic-only recommendation (no cache involvement).
fn heuristic_recommendation(policy: BatchExecutionPolicy) -> RecommendedBatchPolicy {
    RecommendedBatchPolicy {
        policy,
        source: BatchPolicySource::Heuristic,
        cache_key: None,
        cache_path: None,
    }
}

/// Constructs a recommendation backed by a cached or benchmarked result.
fn sourced_recommendation(
    policy: BatchExecutionPolicy,
    source: BatchPolicySource,
    key: String,
    path: Option<String>,
) -> RecommendedBatchPolicy {
    RecommendedBatchPolicy {
        policy,
        source,
        cache_key: Some(key),
        cache_path: path,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Determines the best batch execution policy for a payload.
///
/// Resolution order:
/// 1. On-disk cache hit → return the stored policy (clamped to current memory budget).
/// 2. GPU benchmark sweep → time candidate policies and persist the winner.
/// 3. Heuristic fallback → device-tier defaults when benchmarking cannot run.
pub fn recommended_batch_policy(
    config: &BatchEvalConfig,
    payload: &BatchPayload,
) -> Result<Option<RecommendedBatchPolicy>, String> {
    let shader_key = ShaderKey::for_payload(payload);
    let metal_ctx = context_for_key(shader_key)?;
    let device_name = metal_ctx.device.name().to_string();

    let inflight_depth = preferred_inflight_batches(&device_name);
    let base_limit = preferred_base_limit(&device_name, payload);
    let static_bytes = payload_static_bytes(payload);
    let mem_cap = compute_memory_cap(
        metal_ctx.device.recommended_max_working_set_size(),
        static_bytes,
        inflight_depth,
    );

    let heuristic = BatchExecutionPolicy {
        matches_per_batch: base_limit.min(mem_cap).max(MIN_BATCH_SIZE),
        inflight_batches: inflight_depth,
    };

    let sig = payload_signature(payload);
    let (cache_key, cache_path) = resolve_cache_location(&device_name, &sig);

    // Fast path: reuse a previously benchmarked result from disk.
    if let Some(hit) = load_cached_policy(&device_name, &sig) {
        let restored = BatchExecutionPolicy {
            matches_per_batch: hit.matches_per_batch_cap.min(mem_cap).max(MIN_BATCH_SIZE),
            inflight_batches: hit.inflight_batches.max(1),
        };
        return Ok(Some(sourced_recommendation(
            restored,
            BatchPolicySource::Cached,
            cache_key,
            cache_path,
        )));
    }

    // Prepare payload buffers for a GPU benchmark sweep.
    let Some(prepared) = try_prepare_batch(config, payload)? else {
        return Ok(Some(heuristic_recommendation(heuristic)));
    };

    let candidates = candidate_policies(payload, &device_name, mem_cap);
    if candidates.len() <= 1 {
        return Ok(Some(heuristic_recommendation(heuristic)));
    }

    let synthetic_pairs = generate_benchmark_pairs(payload, &candidates);
    if synthetic_pairs.is_empty() {
        return Ok(Some(heuristic_recommendation(heuristic)));
    }

    // Benchmark all candidates and persist the winner.
    let winner = select_fastest_policy(&prepared, candidates, &synthetic_pairs, heuristic)?;

    persist_cached_policy(&PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name,
        payload_signature: sig,
        matches_per_batch_cap: winner.matches_per_batch,
        inflight_batches: winner.inflight_batches,
    });

    Ok(Some(sourced_recommendation(
        winner,
        BatchPolicySource::Benchmarked,
        cache_key,
        cache_path,
    )))
}
