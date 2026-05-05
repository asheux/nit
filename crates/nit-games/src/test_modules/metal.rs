//! Metal batch evaluator helpers used by the test suite.
//!
//! Lifted out of `tournament/metal/*.rs` so production source carries no
//! test-only code paths. Consumers: `test_modules/tournament_metal.rs`
//! and `test_modules/shared.rs::metal_totals_or_skip`.

use crate::config::{NormalizedConfig, StrategySpec};
use crate::tournament::test_internals::{
    encode_matchup_pairs, match_outcomes_from_scores, try_metal_batch_outcomes_chunked_prepared,
    try_prepare_metal_batch, MatchOutcome, Matchup, PreparedMetalBatch,
};

fn build_test_matchups(index_pairs: &[(usize, usize)]) -> Vec<Matchup> {
    index_pairs
        .iter()
        .enumerate()
        .map(|(match_id, (a_idx, b_idx))| Matchup {
            match_id,
            a_idx: *a_idx,
            b_idx: *b_idx,
            repetition: 0,
        })
        .collect()
}

fn try_metal_batch_outcomes(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchups: &[Matchup],
) -> Result<Option<Vec<MatchOutcome>>, String> {
    let Some(prepared) = try_prepare_metal_batch(config, strategies)? else {
        return Ok(None);
    };
    try_metal_batch_outcomes_prepared(config, strategies, &prepared, matchups)
}

fn try_metal_batch_outcomes_prepared(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    prepared: &PreparedMetalBatch,
    matchups: &[Matchup],
) -> Result<Option<Vec<MatchOutcome>>, String> {
    if matchups.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let gpu_pairs = encode_matchup_pairs(matchups);
    let Some(raw_scores) = nit_metal::try_evaluate_prepared_batch(&prepared.prepared, &gpu_pairs)?
    else {
        return Ok(None);
    };
    Ok(Some(match_outcomes_from_scores(
        config, strategies, matchups, raw_scores,
    )))
}

pub(super) fn metal_batch_totals_for_test(
    config: &NormalizedConfig,
    index_pairs: &[(usize, usize)],
) -> Result<Option<Vec<(i64, i64)>>, String> {
    let test_matchups = build_test_matchups(index_pairs);
    let batch_outcomes = try_metal_batch_outcomes(config, &config.strategies, &test_matchups)?;
    Ok(batch_outcomes.map(|outcomes| {
        outcomes
            .into_iter()
            .map(|o| (o.result.a_total, o.result.b_total))
            .collect()
    }))
}

#[allow(clippy::type_complexity)]
pub(super) fn metal_policy_probe_for_test(
    config: &NormalizedConfig,
    index_pairs: &[(usize, usize)],
    matches_per_batch: usize,
    inflight_depth: usize,
) -> Result<Option<(Vec<(i64, i64)>, std::time::Duration)>, String> {
    let test_matchups = build_test_matchups(index_pairs);
    let Some(mut prepared) = try_prepare_metal_batch(config, &config.strategies)? else {
        return Ok(None);
    };
    prepared.policy.matches_per_batch = matches_per_batch.max(1);
    prepared.policy.inflight_batches = inflight_depth.max(1);

    let clock_start = std::time::Instant::now();
    let chunked_result = try_metal_batch_outcomes_chunked_prepared(
        config,
        &config.strategies,
        &prepared,
        &test_matchups,
    )?;
    let wall_time = clock_start.elapsed();

    let Some((outcomes, _batch_count)) = chunked_result else {
        return Ok(None);
    };

    let score_totals = outcomes
        .into_iter()
        .map(|o| (o.result.a_total, o.result.b_total))
        .collect();

    Ok(Some((score_totals, wall_time)))
}
