//! Batch execution policy: heuristics and GPU benchmarking.

use super::cache::{
    load_cached_policy, persist_cached_policy, policy_cache_key, policy_cache_path,
    policy_cache_root, PolicyCacheEntry, POLICY_CACHE_SCHEMA_VERSION,
};
use super::device::{apple_tier, AppleTier};
use super::dispatch::{
    bytes_per_match_pair, try_begin_prepared_batch, try_finish_prepared_batch, try_prepare_batch,
    PendingBatch, PreparedBatch,
};
use super::shader::{context_for_key, ShaderKey};
use super::MetalResult;
use crate::{
    BatchEvalConfig, BatchExecutionPolicy, BatchPayload, BatchPolicySource, MatchPair,
    RecommendedBatchPolicy, TmTransitionPacked,
};
use std::collections::VecDeque;
use std::mem::size_of;
use std::time::Instant;

const MIN_BATCH_SIZE: usize = 4_096;
const DEFAULT_MEMORY_BUDGET: usize = 64 * 1024 * 1024;

fn payload_static_bytes(payload: &BatchPayload) -> usize {
    let u32_stride = size_of::<u32>();
    match payload {
        BatchPayload::Fsm(fsm) => {
            (fsm.starts.len() + fsm.outputs.len() + fsm.transitions.len()) * u32_stride
        }
        BatchPayload::Ca(ca) => ca.rule_tables.len() * u32_stride,
        BatchPayload::Tm(tm) => {
            tm.start_states.len() * u32_stride
                + tm.transitions.len() * size_of::<TmTransitionPacked>()
        }
    }
}

/// Strategy count with a `.max(1)` guard on the CA divisor so a zero-stride
/// payload still reports one "strategy" for benchmarking purposes.
/// [`BatchPayload::population_count`] intentionally returns 0 in that case.
pub(super) fn payload_strategy_count(payload: &BatchPayload) -> usize {
    match payload {
        BatchPayload::Fsm(fsm) => fsm.starts.len(),
        BatchPayload::Ca(ca) => {
            let entries_per_strategy = ca.rule_table_len.max(1) as usize;
            ca.rule_tables.len() / entries_per_strategy
        }
        BatchPayload::Tm(tm) => tm.start_states.len(),
    }
}

pub(super) fn payload_signature(payload: &BatchPayload) -> String {
    let static_bytes = payload_static_bytes(payload);
    let mib_rounded = static_bytes.div_ceil(1024 * 1024).max(1);
    let static_mib_bucket = mib_rounded.next_power_of_two();
    let strategies = payload_strategy_count(payload);

    match payload {
        BatchPayload::Fsm(fsm) => format!(
            "fsm_s{}_a{}_n{}_static{}mib",
            fsm.states, fsm.alphabet, strategies, static_mib_bucket
        ),
        BatchPayload::Ca(ca) => format!(
            "ca_sym{}_twor{}_steps{}_table{}_n{}_static{}mib",
            ca.symbols, ca.two_r, ca.steps, ca.rule_table_len, strategies, static_mib_bucket
        ),
        BatchPayload::Tm(tm) => format!(
            "tm_s{}_sym{}_steps{}_n{}_static{}mib",
            tm.states, tm.symbols, tm.max_steps, strategies, static_mib_bucket
        ),
    }
}

/// Higher-end Apple Silicon tiers benefit from deeper dispatch queues.
pub(super) fn preferred_inflight_batches(gpu_device_name: &str) -> usize {
    match apple_tier(gpu_device_name) {
        AppleTier::Ultra => 5,
        AppleTier::Max => 4,
        AppleTier::Pro => 3,
        AppleTier::Base => 2,
    }
}

/// FSM kernels are lightweight per-pair (larger batches); TM kernels are
/// heavier (smaller batches to stay within budget).
pub(super) fn preferred_base_limit(gpu_device_name: &str, payload: &BatchPayload) -> usize {
    let tier = apple_tier(gpu_device_name);
    match payload {
        BatchPayload::Fsm(_) if matches!(tier, AppleTier::Max | AppleTier::Ultra) => 262_144,
        BatchPayload::Fsm(_) => 131_072,
        BatchPayload::Ca(_) => 65_536,
        BatchPayload::Tm(_) => 32_768,
    }
}

struct PayloadBatchLimits {
    candidate_ceiling: usize,
    benchmark_floor: usize,
    benchmark_ceiling: usize,
}

impl PayloadBatchLimits {
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

/// Generates a single large pool of synthetic pairs (each strategy paired
/// with its mirror) that every candidate policy re-uses during timing.
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

/// Simulates real dispatch: submits chunks and retires the oldest when the
/// inflight depth is saturated, so depth and batch-size interact as they will
/// in production.
fn time_benchmark_trial(
    prepared: &PreparedBatch,
    policy: BatchExecutionPolicy,
    pairs: &[MatchPair],
) -> MetalResult<f64> {
    let chunk_size = policy.matches_per_batch.max(1);
    let depth_cap = policy.inflight_batches.max(1);
    let started = Instant::now();

    let mut inflight: VecDeque<PendingBatch> = VecDeque::with_capacity(depth_cap + 1);

    for chunk in pairs.chunks(chunk_size) {
        let submitted = try_begin_prepared_batch(prepared, chunk)?
            .ok_or("Metal batch benchmark: dispatch failed to begin")?;
        inflight.push_back(submitted);

        if inflight.len() <= depth_cap {
            continue;
        }
        let retired = inflight.pop_front().expect("inflight is non-empty");
        let _ = try_finish_prepared_batch(retired)?;
    }

    for retired in inflight {
        let _ = try_finish_prepared_batch(retired)?;
    }
    Ok(started.elapsed().as_secs_f64())
}

fn select_fastest_policy(
    prepared: &PreparedBatch,
    candidates: Vec<BatchExecutionPolicy>,
    pairs: &[MatchPair],
    fallback: BatchExecutionPolicy,
) -> MetalResult<BatchExecutionPolicy> {
    let timed: Vec<(BatchExecutionPolicy, f64)> = candidates
        .into_iter()
        .map(|policy| Ok((policy, time_benchmark_trial(prepared, policy, pairs)?)))
        .collect::<MetalResult<_>>()?;

    Ok(timed
        .into_iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(policy, _)| policy)
        .unwrap_or(fallback))
}

/// Derives budget from device working set, subtracts static payload overhead,
/// then divides by per-pair buffer size so the cap is the maximum number of
/// in-flight pairs the GPU can hold without thrashing.
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

fn resolve_cache_location(gpu_device_name: &str, payload_sig: &str) -> (String, Option<String>) {
    let key = policy_cache_key(gpu_device_name, payload_sig);
    let path = policy_cache_root().map(|root| {
        policy_cache_path(&root, gpu_device_name, payload_sig)
            .to_string_lossy()
            .into_owned()
    });
    (key, path)
}

struct PolicyContext {
    device_name: String,
    mem_cap: usize,
    heuristic: BatchExecutionPolicy,
}

fn prepare_policy_context(payload: &BatchPayload) -> MetalResult<PolicyContext> {
    let shader_key = ShaderKey::for_payload(payload);
    let ctx = context_for_key(shader_key)?;
    let device_name = ctx.device.name().to_string();

    let inflight_depth = preferred_inflight_batches(&device_name);
    let base_limit = preferred_base_limit(&device_name, payload);
    let mem_cap = compute_memory_cap(
        ctx.device.recommended_max_working_set_size(),
        payload_static_bytes(payload),
        inflight_depth,
    );

    Ok(PolicyContext {
        device_name,
        mem_cap,
        heuristic: BatchExecutionPolicy {
            matches_per_batch: base_limit.min(mem_cap).max(MIN_BATCH_SIZE),
            inflight_batches: inflight_depth,
        },
    })
}

fn cached_recommendation(
    hit: PolicyCacheEntry,
    mem_cap: usize,
    cache_key: String,
    cache_path: Option<String>,
) -> RecommendedBatchPolicy {
    RecommendedBatchPolicy {
        policy: BatchExecutionPolicy {
            matches_per_batch: hit.matches_per_batch_cap.min(mem_cap).max(MIN_BATCH_SIZE),
            inflight_batches: hit.inflight_batches.max(1),
        },
        source: BatchPolicySource::Cached,
        cache_key: Some(cache_key),
        cache_path,
    }
}

/// Resolution order: cache hit → GPU benchmark sweep → heuristic fallback.
/// Each step falls back to the heuristic rather than erroring, so transient
/// GPU issues still produce a usable policy.
pub fn recommended_batch_policy(
    config: &BatchEvalConfig,
    payload: &BatchPayload,
) -> MetalResult<Option<RecommendedBatchPolicy>> {
    let ctx = prepare_policy_context(payload)?;
    let sig = payload_signature(payload);
    let (cache_key, cache_path) = resolve_cache_location(&ctx.device_name, &sig);

    if let Some(hit) = load_cached_policy(&ctx.device_name, &sig) {
        return Ok(Some(cached_recommendation(
            hit,
            ctx.mem_cap,
            cache_key,
            cache_path,
        )));
    }

    let Some(prepared) = try_prepare_batch(config, payload)? else {
        return Ok(Some(RecommendedBatchPolicy {
            policy: ctx.heuristic,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    };

    let candidates = candidate_policies(payload, &ctx.device_name, ctx.mem_cap);
    let synthetic = if candidates.len() > 1 {
        generate_benchmark_pairs(payload, &candidates)
    } else {
        Vec::new()
    };

    if synthetic.is_empty() {
        return Ok(Some(RecommendedBatchPolicy {
            policy: ctx.heuristic,
            source: BatchPolicySource::Heuristic,
            cache_key: None,
            cache_path: None,
        }));
    }

    let winner = select_fastest_policy(&prepared, candidates, &synthetic, ctx.heuristic)?;

    persist_cached_policy(&PolicyCacheEntry {
        schema_version: POLICY_CACHE_SCHEMA_VERSION,
        device_name: ctx.device_name,
        payload_signature: sig,
        matches_per_batch_cap: winner.matches_per_batch,
        inflight_batches: winner.inflight_batches,
    });

    Ok(Some(RecommendedBatchPolicy {
        policy: winner,
        source: BatchPolicySource::Benchmarked,
        cache_key: Some(cache_key),
        cache_path,
    }))
}
