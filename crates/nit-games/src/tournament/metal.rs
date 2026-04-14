use super::types::{MatchOutcome, MatchResult, Matchup, PreparedMetalBatch};
use crate::config::{NormalizedConfig, StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::{TmMove, TmRunStats};
use nit_metal::{
    BatchEvalConfig, BatchExecutionPolicy, BatchPayload, BatchPolicySource, CaBatch, FsmBatch,
    MatchPair, TmBatch,
};
use std::collections::VecDeque;

fn ensure_uniform<T: PartialEq>(slot: &mut Option<T>, incoming: T) -> Option<()> {
    match slot {
        Some(existing) if *existing != incoming => None,
        slot_ref @ None => {
            *slot_ref = Some(incoming);
            Some(())
        }
        _ => Some(()),
    }
}

pub(super) fn adjusted_total_for_match(
    raw_total: i64,
    strategy: &StrategySpec,
    round_count: u32,
    tm_run_stats: Option<&TmRunStats>,
    cost: &crate::config::ComplexityCostConfig,
) -> f64 {
    if !cost.enabled {
        return raw_total as f64;
    }

    let penalty = compute_complexity_penalty(strategy, round_count, tm_run_stats, cost);
    raw_total as f64 - penalty
}

fn compute_complexity_penalty(
    strategy: &StrategySpec,
    round_count: u32,
    tm_run_stats: Option<&TmRunStats>,
    cost: &crate::config::ComplexityCostConfig,
) -> f64 {
    match &strategy.kind {
        StrategySpecKind::OneSidedTm { .. } if cost.tm_step_cost != 0.0 => {
            let total_steps = tm_run_stats.map_or(0.0, |s| s.steps as f64);
            cost.tm_step_cost * total_steps
        }

        StrategySpecKind::Fsm {
            num_states,
            outputs,
            ..
        } if cost.fsm_state_cost != 0.0 => {
            let state_count = if *num_states > 0 {
                *num_states
            } else {
                outputs.len()
            };
            cost.fsm_state_cost * state_count as f64 * round_count as f64
        }

        _ => 0.0,
    }
}

fn has_tm_step_cost_conflict(config: &NormalizedConfig, strategies: &[StrategySpec]) -> bool {
    config.engine.complexity_cost.enabled
        && config.engine.complexity_cost.tm_step_cost != 0.0
        && strategies
            .iter()
            .all(|spec| matches!(spec.kind, StrategySpecKind::OneSidedTm { .. }))
}

fn move_dir_code(direction: TmMove) -> u32 {
    match direction {
        TmMove::Left => 0,
        TmMove::Right => 1,
        TmMove::Stay => 2,
    }
}

fn build_metal_batch_payload(strategies: &[StrategySpec]) -> Option<BatchPayload> {
    let first = strategies.first()?;
    match &first.kind {
        StrategySpecKind::Fsm { .. } => build_metal_fsm_payload(strategies).map(BatchPayload::Fsm),
        StrategySpecKind::Ca { .. } => build_metal_ca_payload(strategies).map(BatchPayload::Ca),
        StrategySpecKind::OneSidedTm { .. } => {
            build_metal_tm_payload(strategies).map(BatchPayload::Tm)
        }
    }
}

struct ValidatedFsm<'a> {
    state_count: usize,
    alphabet_size: usize,
    start_state: usize,
    outputs: &'a [Action],
    transition_table: &'a [Vec<usize>],
}

fn validate_fsm_spec(spec: &StrategySpec) -> Option<ValidatedFsm<'_>> {
    let StrategySpecKind::Fsm {
        num_states,
        start_state,
        outputs: state_outputs,
        input_mode,
        transitions: transition_table,
        ..
    } = &spec.kind
    else {
        return None;
    };

    let effective_mode = input_mode.unwrap_or(crate::strategy::InputMode::OpponentLastAction);
    if !matches!(
        effective_mode,
        crate::strategy::InputMode::OpponentLastAction
    ) {
        return None;
    }

    let state_count = (*num_states).max(state_outputs.len());
    let alphabet_size = transition_table.first().map_or(0, Vec::len);

    if alphabet_size != 2
        || state_count == 0
        || transition_table.len() != state_count
        || *start_state >= state_count
    {
        return None;
    }

    if transition_table
        .iter()
        .any(|row| row.len() != alphabet_size)
    {
        return None;
    }

    Some(ValidatedFsm {
        state_count,
        alphabet_size,
        start_state: *start_state,
        outputs: state_outputs,
        transition_table,
    })
}

fn build_metal_fsm_payload(strategies: &[StrategySpec]) -> Option<FsmBatch> {
    let mut uniform_states: Option<usize> = None;
    let mut uniform_alphabet: Option<usize> = None;
    let mut start_indices = Vec::with_capacity(strategies.len());
    let mut flat_outputs = Vec::new();
    let mut flat_transitions = Vec::new();

    for spec in strategies {
        let fsm = validate_fsm_spec(spec)?;
        let ValidatedFsm {
            state_count,
            alphabet_size,
            start_state: start,
            outputs,
            transition_table: table,
        } = fsm;

        ensure_uniform(&mut uniform_states, state_count)?;
        ensure_uniform(&mut uniform_alphabet, alphabet_size)?;

        start_indices.push(start as u32);

        flat_outputs.extend(outputs.iter().map(|action| match action {
            Action::Cooperate => 0u32,
            Action::Defect => 1u32,
        }));
        let padding_needed = state_count.saturating_sub(outputs.len());
        flat_outputs.extend(std::iter::repeat_n(0u32, padding_needed));

        for row in table {
            for &next_state in row {
                if next_state >= state_count {
                    return None;
                }
                flat_transitions.push(next_state as u32);
            }
        }
    }

    Some(FsmBatch {
        states: uniform_states? as u32,
        alphabet: uniform_alphabet? as u32,
        starts: start_indices,
        outputs: flat_outputs,
        transitions: flat_transitions,
    })
}

fn build_metal_ca_payload(strategies: &[StrategySpec]) -> Option<CaBatch> {
    let mut uniform_symbols: Option<u32> = None;
    let mut uniform_two_r: Option<u32> = None;
    let mut uniform_steps: Option<u32> = None;
    let mut uniform_table_len: Option<u32> = None;
    let mut flat_rule_tables = Vec::new();

    for spec in strategies {
        let StrategySpecKind::Ca { n, k, r, t } = &spec.kind else {
            return None;
        };

        let derived_two_r = (*r * 2.0).round() as u32;
        if ((*r * 2.0) - derived_two_r as f32).abs() > 0.0001 {
            return None;
        }

        ensure_uniform(&mut uniform_symbols, *k as u32)?;
        ensure_uniform(&mut uniform_two_r, derived_two_r)?;
        ensure_uniform(&mut uniform_steps, *t)?;

        let decoded_table = crate::strategy::decode_ca_rule_table(*n, *k, derived_two_r);
        ensure_uniform(&mut uniform_table_len, decoded_table.len() as u32)?;

        flat_rule_tables.extend(decoded_table.into_iter().map(u32::from));
    }

    Some(CaBatch {
        symbols: uniform_symbols?,
        two_r: uniform_two_r?,
        steps: uniform_steps?,
        rule_table_len: uniform_table_len?,
        rule_tables: flat_rule_tables,
    })
}

fn build_metal_tm_payload(strategies: &[StrategySpec]) -> Option<TmBatch> {
    let mut uniform_states: Option<u32> = None;
    let mut uniform_symbols: Option<u32> = None;
    let mut uniform_blank: Option<u32> = None;
    let mut uniform_max_steps: Option<u32> = None;
    let mut start_states = Vec::with_capacity(strategies.len());
    let mut packed_transitions = Vec::new();

    for spec in strategies {
        let StrategySpecKind::OneSidedTm {
            states: tm_state_count,
            symbols: tm_symbol_count,
            start_state,
            blank: tm_blank_symbol,
            max_steps_per_round,
            transitions: tm_rules,
            ..
        } = &spec.kind
        else {
            return None;
        };

        ensure_uniform(&mut uniform_states, *tm_state_count as u32)?;
        ensure_uniform(&mut uniform_symbols, *tm_symbol_count as u32)?;
        ensure_uniform(&mut uniform_blank, *tm_blank_symbol as u32)?;
        ensure_uniform(&mut uniform_max_steps, *max_steps_per_round)?;

        let expected_entries = (*tm_state_count as usize).saturating_mul(*tm_symbol_count as usize);
        if tm_rules.len() != expected_entries {
            return None;
        }

        let bounds_violated = *start_state > *tm_state_count
            || tm_rules
                .iter()
                .any(|rule| rule.write >= *tm_symbol_count || rule.next > *tm_state_count);
        if bounds_violated {
            return None;
        }

        start_states.push(*start_state as u32);
        packed_transitions.extend(tm_rules.iter().map(|rule| nit_metal::TmTransitionPacked {
            write: u32::from(rule.write),
            move_dir: move_dir_code(rule.move_dir),
            next: u32::from(rule.next),
        }));
    }

    Some(TmBatch {
        states: uniform_states?,
        symbols: uniform_symbols?,
        blank: uniform_blank?,
        max_steps: uniform_max_steps?,
        start_states,
        transitions: packed_transitions,
    })
}

pub(super) fn metal_batch_eval_config(config: &NormalizedConfig) -> BatchEvalConfig {
    let (timeout_lose, timeout_win) = config.payoff.min_max();
    BatchEvalConfig {
        rounds: config.rounds,
        payoff: config.payoff.matrix,
        timeout_lose,
        timeout_win,
    }
}

const SMALL_METAL_WORKLOAD_MATCHUPS: usize = 4_096;

pub(super) fn prepare_metal_batch_inputs(
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

fn try_prepare_metal_batch(
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

pub(super) fn try_prepare_metal_batch_for_workload(
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

pub(super) fn metal_batch_decline_reason(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchup_count: usize,
) -> Option<String> {
    if matchup_count == 0 {
        return Some("no matchups to evaluate".into());
    }
    if !config.engine.fast_eval {
        return Some("`engine.fast_eval = false` disables Metal batch evaluation".into());
    }
    if !config.engine.accelerator.allows_metal() {
        return Some("accelerator mode is set to CPU".into());
    }
    if config.noise != 0.0 {
        return Some("non-zero noise disables Metal batch evaluation".into());
    }
    if strategies.is_empty() {
        return None;
    }

    if has_tm_step_cost_conflict(config, strategies) {
        return Some("TM complexity penalties are not supported on the Metal path".into());
    }

    if build_metal_batch_payload(strategies).is_none() {
        return Some(
            "Metal batch evaluation requires a homogeneous FSM, CA, or TM roster \
             with shared structural parameters."
                .into(),
        );
    }

    None
}

fn match_outcomes_from_scores(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchups: &[Matchup],
    gpu_scores: Vec<nit_metal::ScorePair>,
) -> Vec<MatchOutcome> {
    let cost = &config.engine.complexity_cost;
    matchups
        .iter()
        .zip(gpu_scores)
        .map(|(matchup, score_pair)| {
            let spec_a = &strategies[matchup.a_idx];
            let spec_b = &strategies[matchup.b_idx];
            MatchOutcome {
                result: MatchResult {
                    a_idx: matchup.a_idx,
                    b_idx: matchup.b_idx,
                    rounds: config.rounds,
                    a_total: score_pair.a_total,
                    b_total: score_pair.b_total,
                    a_adjusted_total: adjusted_total_for_match(
                        score_pair.a_total,
                        spec_a,
                        config.rounds,
                        None,
                        cost,
                    ),
                    b_adjusted_total: adjusted_total_for_match(
                        score_pair.b_total,
                        spec_b,
                        config.rounds,
                        None,
                        cost,
                    ),
                    repetition: matchup.repetition,
                    match_id: matchup.match_id,
                },
                a_crashed: false,
                b_crashed: false,
                a_tm_stats: None,
                b_tm_stats: None,
                last_round: None,
            }
        })
        .collect()
}

pub fn accelerator_preflight(config: &NormalizedConfig) -> Result<(), String> {
    if !config.engine.accelerator.requires_metal() {
        return Ok(());
    }
    if !config.engine.fast_eval {
        return Err("Metal accelerator requires `engine.fast_eval = true`.".into());
    }
    if config.noise != 0.0 {
        return Err("Metal accelerator requires `noise = 0.0`.".into());
    }
    if config.strategies.is_empty() {
        return Ok(());
    }

    if has_tm_step_cost_conflict(config, &config.strategies) {
        return Err(
            "Metal accelerator does not support TM complexity penalties; \
             disable `engine.complexity_cost.tm_step_cost` or use `accelerator = \"auto\"`."
                .into(),
        );
    }

    let payload = build_metal_batch_payload(&config.strategies).ok_or_else(|| {
        "Metal accelerator requires a homogeneous FSM, CA, or TM roster \
         that the Metal batch evaluator can encode."
            .to_string()
    })?;

    let eval = metal_batch_eval_config(config);
    let prepared = nit_metal::try_prepare_batch(&eval, &payload)?.ok_or_else(|| {
        "Metal accelerator was requested, but this run is not supported \
         by the active Metal backend."
            .to_string()
    })?;

    let probe_pair = [MatchPair { a_idx: 0, b_idx: 0 }];
    match nit_metal::try_evaluate_prepared_batch(&prepared, &probe_pair) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(
            "Metal accelerator was requested, but this run is not supported \
             by the active Metal backend."
                .into(),
        ),
        Err(err) => Err(format!("Metal accelerator unavailable: {err}")),
    }
}

pub fn accelerator_run_preflight(
    config: &NormalizedConfig,
    event_logging: bool,
    history_logging: bool,
    match_history_previews: bool,
) -> Result<(), String> {
    if !config.engine.accelerator.requires_metal() {
        return Ok(());
    }

    let blockers = collect_metal_blockers(event_logging, history_logging, match_history_previews);
    if !blockers.is_empty() {
        let formatted = format_blocker_list(&blockers);
        return Err(format!(
            "Metal accelerator was requested, but {formatted} currently requires the CPU path. \
             Disable those features or use `accelerator = \"auto\"`."
        ));
    }

    accelerator_preflight(config)
}

fn collect_metal_blockers(
    event_logging: bool,
    history_logging: bool,
    match_history_previews: bool,
) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if event_logging {
        blockers.push("event logging");
    }
    if history_logging {
        blockers.push("history logging");
    }
    if match_history_previews {
        blockers.push("interactive match history previews");
    }
    blockers
}

fn format_blocker_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [single] => single.to_string(),
        [left, right] => format!("{left} and {right}"),
        _ => {
            let (init, last) = items.split_at(items.len() - 1);
            format!("{}, and {}", init.join(", "), last[0])
        }
    }
}

pub(super) fn try_metal_batch_outcomes_chunked_prepared(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    prepared: &PreparedMetalBatch,
    matchups: &[Matchup],
) -> Result<Option<(Vec<MatchOutcome>, usize)>, String> {
    if matchups.is_empty() {
        return Ok(Some((Vec::new(), 0)));
    }

    let mut collected_outcomes = Vec::with_capacity(matchups.len());
    let mut dispatch_count = 0usize;
    let mut inflight: VecDeque<InflightChunk<'_>> = VecDeque::new();

    for chunk in matchups.chunks(prepared.policy.matches_per_batch) {
        let gpu_pairs = encode_matchup_pairs(chunk);
        let pending = match nit_metal::try_begin_prepared_batch(&prepared.prepared, &gpu_pairs)? {
            Some(batch) => batch,
            None => return Ok(None),
        };
        dispatch_count += 1;

        inflight.push_back(InflightChunk {
            source_matchups: chunk,
            pending,
        });

        if inflight.len() >= prepared.policy.inflight_batches {
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

fn encode_matchup_pairs(matchups: &[Matchup]) -> Vec<MatchPair> {
    matchups
        .iter()
        .map(|m| MatchPair {
            a_idx: m.a_idx as u32,
            b_idx: m.b_idx as u32,
        })
        .collect()
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn metal_batch_totals_for_test(
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

#[cfg(test)]
#[allow(clippy::type_complexity)]
pub(crate) fn metal_policy_probe_for_test(
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
