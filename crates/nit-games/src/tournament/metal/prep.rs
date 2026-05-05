use super::has_tm_step_cost_conflict;
use super::payload::{build_metal_batch_payload, metal_batch_eval_config};
use crate::config::{NormalizedConfig, StrategySpec};
use crate::tournament::types::PreparedMetalBatch;
use nit_metal::{BatchEvalConfig, BatchExecutionPolicy, BatchPayload, BatchPolicySource};

const SMALL_METAL_WORKLOAD_MATCHUPS: usize = 4_096;

fn prepare_metal_batch_inputs(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
) -> Result<Option<(BatchEvalConfig, BatchPayload)>, String> {
    if !config.engine.fast_eval || !config.engine.accelerator.allows_metal() || config.noise != 0.0
    {
        return Ok(None);
    }
    if strategies.is_empty() {
        return Ok(None);
    }
    if has_tm_step_cost_conflict(config, strategies) {
        return Ok(None);
    }
    let Some(payload) = build_metal_batch_payload(strategies) else {
        return Ok(None);
    };
    Ok(Some((metal_batch_eval_config(config), payload)))
}

pub(crate) fn try_prepare_metal_batch(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
) -> Result<Option<PreparedMetalBatch>, String> {
    let Some((eval, payload)) = prepare_metal_batch_inputs(config, strategies)? else {
        return Ok(None);
    };
    let Some(policy_report) = nit_metal::recommended_batch_policy(&eval, &payload)? else {
        return Ok(None);
    };
    let Some(prepared_handle) = nit_metal::try_prepare_batch(&eval, &payload)? else {
        return Ok(None);
    };

    Ok(Some(PreparedMetalBatch {
        prepared: prepared_handle,
        policy: policy_report.policy,
        policy_source: policy_report.source,
        policy_cache_key: policy_report.cache_key,
        policy_cache_path: policy_report.cache_path,
    }))
}

fn try_prepare_metal_batch_for_matchups(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchup_count: usize,
) -> Result<Option<PreparedMetalBatch>, String> {
    let Some((eval, payload)) = prepare_metal_batch_inputs(config, strategies)? else {
        return Ok(None);
    };
    let Some(prepared_handle) = nit_metal::try_prepare_batch(&eval, &payload)? else {
        return Ok(None);
    };

    Ok(Some(PreparedMetalBatch {
        prepared: prepared_handle,
        policy: BatchExecutionPolicy {
            matches_per_batch: matchup_count.clamp(1, 4_096),
            inflight_batches: 1,
        },
        policy_source: BatchPolicySource::Heuristic,
        policy_cache_key: None,
        policy_cache_path: None,
    }))
}

pub(crate) fn try_prepare_metal_batch_for_workload(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchup_count: usize,
) -> Result<Option<PreparedMetalBatch>, String> {
    if matchup_count <= SMALL_METAL_WORKLOAD_MATCHUPS {
        try_prepare_metal_batch_for_matchups(config, strategies, matchup_count)
    } else {
        try_prepare_metal_batch(config, strategies)
    }
}
