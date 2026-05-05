use super::adjusted_total_for_match;
use crate::config::{NormalizedConfig, StrategySpec};
use crate::tournament::types::{MatchOutcome, MatchResult, Matchup, PreparedMetalBatch};
use nit_metal::MatchPair;
use std::collections::VecDeque;

pub(crate) fn encode_matchup_pairs(matchups: &[Matchup]) -> Vec<MatchPair> {
    matchups
        .iter()
        .map(|m| MatchPair {
            a_idx: m.a_idx as u32,
            b_idx: m.b_idx as u32,
        })
        .collect()
}

pub(crate) fn match_outcomes_from_scores(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchups: &[Matchup],
    gpu_scores: Vec<nit_metal::ScorePair>,
) -> Vec<MatchOutcome> {
    let penalty_cost = &config.engine.complexity_cost;
    let total_rounds = config.rounds;
    let adjust = |raw_score: i64, owning_idx: usize| {
        adjusted_total_for_match(
            raw_score,
            &strategies[owning_idx],
            total_rounds,
            None,
            penalty_cost,
        )
    };
    matchups
        .iter()
        .zip(gpu_scores)
        .map(|(pairing, gpu_pair)| MatchOutcome {
            result: MatchResult {
                a_idx: pairing.a_idx,
                b_idx: pairing.b_idx,
                rounds: total_rounds,
                a_total: gpu_pair.a_total,
                b_total: gpu_pair.b_total,
                a_adjusted_total: adjust(gpu_pair.a_total, pairing.a_idx),
                b_adjusted_total: adjust(gpu_pair.b_total, pairing.b_idx),
                repetition: pairing.repetition,
                match_id: pairing.match_id,
            },
            a_crashed: false,
            b_crashed: false,
            a_tm_stats: None,
            b_tm_stats: None,
            last_round: None,
        })
        .collect()
}

pub(crate) fn try_metal_batch_outcomes_chunked_prepared(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    prepared: &PreparedMetalBatch,
    matchups: &[Matchup],
) -> Result<Option<(Vec<MatchOutcome>, usize)>, String> {
    if matchups.is_empty() {
        return Ok(Some((Vec::new(), 0)));
    }

    let chunk_size = prepared.policy.matches_per_batch;
    let max_inflight = prepared.policy.inflight_batches;
    let mut collected_outcomes = Vec::with_capacity(matchups.len());
    let mut dispatch_count = 0usize;
    let mut inflight: VecDeque<InflightChunk<'_>> = VecDeque::new();

    for chunk in matchups.chunks(chunk_size) {
        let gpu_pairs = encode_matchup_pairs(chunk);
        let Some(pending) = nit_metal::try_begin_prepared_batch(&prepared.prepared, &gpu_pairs)?
        else {
            return Ok(None);
        };
        dispatch_count += 1;
        inflight.push_back(InflightChunk {
            source_matchups: chunk,
            pending,
        });
        if inflight.len() >= max_inflight {
            drain_one_inflight(&mut inflight, config, strategies, &mut collected_outcomes)?;
        }
    }

    while !inflight.is_empty() {
        drain_one_inflight(&mut inflight, config, strategies, &mut collected_outcomes)?;
    }

    Ok(Some((collected_outcomes, dispatch_count)))
}

struct InflightChunk<'a> {
    source_matchups: &'a [Matchup],
    pending: nit_metal::PendingBatch,
}

fn drain_one_inflight(
    inflight: &mut VecDeque<InflightChunk<'_>>,
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    outcomes: &mut Vec<MatchOutcome>,
) -> Result<(), String> {
    let ready = inflight.pop_front().expect("non-empty inflight queue");
    let finished_scores = nit_metal::try_finish_prepared_batch(ready.pending)?;
    outcomes.extend(match_outcomes_from_scores(
        config,
        strategies,
        ready.source_matchups,
        finished_scores,
    ));
    Ok(())
}
