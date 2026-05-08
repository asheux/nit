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

fn heuristic_policy(matchup_count: usize) -> BatchExecutionPolicy {
    BatchExecutionPolicy {
        matches_per_batch: matchup_count.clamp(1, 4_096),
        inflight_batches: 1,
    }
}

// `heuristic_matchup_count = Some(_)` skips the recommended-policy lookup and
// uses an inline single-batch policy sized to the workload — the small-workload
// path's avoid-the-cache shortcut. None forces the full policy lookup so the
// per-shape cache populates.
fn try_prepare_metal_batch_inner(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    heuristic_matchup_count: Option<usize>,
) -> Result<Option<PreparedMetalBatch>, String> {
    let Some((eval, payload)) = prepare_metal_batch_inputs(config, strategies)? else {
        return Ok(None);
    };

    if let Some(count) = heuristic_matchup_count {
        let Some(prepared) = nit_metal::try_prepare_batch(&eval, &payload)? else {
            return Ok(None);
        };
        return Ok(Some(PreparedMetalBatch {
            prepared,
            policy: heuristic_policy(count),
            policy_source: BatchPolicySource::Heuristic,
            policy_cache_key: None,
            policy_cache_path: None,
        }));
    }

    let Some(report) = nit_metal::recommended_batch_policy(&eval, &payload)? else {
        return Ok(None);
    };
    let Some(prepared) = nit_metal::try_prepare_batch(&eval, &payload)? else {
        return Ok(None);
    };
    Ok(Some(PreparedMetalBatch {
        prepared,
        policy: report.policy,
        policy_source: report.source,
        policy_cache_key: report.cache_key,
        policy_cache_path: report.cache_path,
    }))
}

pub(crate) fn try_prepare_metal_batch(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
) -> Result<Option<PreparedMetalBatch>, String> {
    try_prepare_metal_batch_inner(config, strategies, None)
}

pub(crate) fn try_prepare_metal_batch_for_workload(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchup_count: usize,
) -> Result<Option<PreparedMetalBatch>, String> {
    if matchup_count <= SMALL_METAL_WORKLOAD_MATCHUPS {
        try_prepare_metal_batch_inner(config, strategies, Some(matchup_count))
    } else {
        try_prepare_metal_batch(config, strategies)
    }
}
