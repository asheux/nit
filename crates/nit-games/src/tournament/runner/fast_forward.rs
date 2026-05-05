use super::{TournamentRunner, SNAPSHOT_REFRESH_MS};
use crate::events::{EventWriter, GameEvent};
use crate::history_log::MatchHistory;
use crate::tournament::metal::{
    metal_batch_decline_reason, try_metal_batch_outcomes_chunked_prepared,
    try_prepare_metal_batch_for_workload,
};
use crate::tournament::session::run_match_core;
use crate::tournament::types::{
    run_with_parallelism, MatchOutcome, Matchup, MetalBatchState, Parallelism,
};
use rayon::prelude::*;
use std::time::{Duration, Instant};

impl TournamentRunner {
    pub(super) fn fast_forward_allowed(&self) -> bool {
        self.current.is_none()
            && self.config.engine.fast_eval
            && self.config.noise == 0.0
            && self.event_writer.is_none()
            && self.history_writer.is_none()
            && !self.collect_match_history_previews
    }

    fn ensure_metal_batch(&mut self) {
        if !matches!(self.metal_batch, MetalBatchState::Uninitialized) {
            return;
        }
        let remaining_match_count = self.schedule.len().saturating_sub(self.match_index);
        let preparation_result = try_prepare_metal_batch_for_workload(
            &self.config,
            &self.strategies,
            remaining_match_count,
        );
        match preparation_result {
            Ok(Some(prepared_pipeline)) => {
                self.runtime.note_metal_policy(
                    prepared_pipeline.policy.matches_per_batch,
                    prepared_pipeline.policy.inflight_batches,
                    prepared_pipeline.policy_source,
                    prepared_pipeline.policy_cache_key.clone(),
                    prepared_pipeline.policy_cache_path.clone(),
                );
                self.metal_batch = MetalBatchState::Prepared(prepared_pipeline);
            }
            Ok(None) => {
                self.note_metal_decline_reason();
                self.metal_batch = MetalBatchState::Unavailable;
            }
            Err(preparation_error) => {
                if self.config.engine.accelerator.allows_metal() {
                    self.runtime.note_metal_fallback_reason(format!(
                        "Metal backend error: {preparation_error}"
                    ));
                }
                self.metal_batch = MetalBatchState::Unavailable;
            }
        }
    }

    fn note_metal_decline_reason(&mut self) {
        if !self.config.engine.accelerator.allows_metal() {
            return;
        }
        let decline_reason = metal_batch_decline_reason(&self.config, &self.strategies, 1)
            .unwrap_or_else(|| "Metal batch evaluator declined this workload".into());
        self.runtime.note_metal_fallback_reason(decline_reason);
    }

    pub(super) fn try_fast_forward_matches(&mut self, remaining_step_budget: &mut u32) {
        if !self.fast_forward_allowed() {
            return;
        }
        let rounds_per_match = self.config.rounds.max(1);
        let affordable_match_count = (*remaining_step_budget / rounds_per_match) as usize;
        if affordable_match_count == 0 {
            return;
        }
        let schedulable_match_count = self.schedule.len().saturating_sub(self.match_index);
        let batch_match_count = affordable_match_count.min(schedulable_match_count);
        if batch_match_count == 0 {
            return;
        }

        let scheduled_total = self.schedule.len();
        let batch_matchups = self.schedule.matchups(self.match_index, batch_match_count);
        if batch_matchups.is_empty() {
            return;
        }
        self.ensure_metal_batch();
        let tournament_config = &self.config;
        let strategy_roster = &self.strategies;
        let match_seed_deriver = &self.seed_deriver;
        let precompiled_fast_models = &self.fast_models;
        let evaluate_single_matchup = |target_matchup: &Matchup, fast_eval_enabled: bool| {
            let mut discard_event = |_event: GameEvent| {};
            let mut discard_history = |_record: MatchHistory| {};
            run_match_core(
                target_matchup,
                tournament_config,
                strategy_roster,
                match_seed_deriver,
                Some(precompiled_fast_models),
                fast_eval_enabled,
                scheduled_total,
                false,
                false,
                &mut discard_event,
                false,
                &mut discard_history,
                false,
            )
        };
        let pending_matchup_slice = batch_matchups.as_slice();
        let evaluate_parallel = || {
            pending_matchup_slice
                .par_iter()
                .map(|queued_matchup| evaluate_single_matchup(queued_matchup, true))
                .collect::<Vec<_>>()
        };
        let gpu_evaluation_result = match &self.metal_batch {
            MetalBatchState::Prepared(prepared_pipeline) => {
                try_metal_batch_outcomes_chunked_prepared(
                    tournament_config,
                    strategy_roster,
                    prepared_pipeline,
                    pending_matchup_slice,
                )
            }
            MetalBatchState::Uninitialized | MetalBatchState::Unavailable => Ok(None),
        };
        let (batch_outcomes, executed_on_gpu) = match gpu_evaluation_result {
            Ok(Some((gpu_outcomes, metal_batch_count))) => {
                self.runtime
                    .note_metal_batches(metal_batch_count, pending_matchup_slice.len());
                (gpu_outcomes, true)
            }
            other => {
                if let Err(gpu_error) = other {
                    if !pending_matchup_slice.is_empty()
                        && self.config.engine.accelerator.allows_metal()
                    {
                        self.runtime.note_metal_fallback_reason(format!(
                            "Metal backend error: {gpu_error}"
                        ));
                        self.metal_batch = MetalBatchState::Unavailable;
                    }
                }
                let parallelism_mode = Parallelism::from_config(&self.config.engine.parallelism);
                let cpu_outcomes = if matches!(parallelism_mode, Parallelism::Off) {
                    pending_matchup_slice
                        .iter()
                        .map(|queued_matchup| evaluate_single_matchup(queued_matchup, true))
                        .collect()
                } else {
                    run_with_parallelism(parallelism_mode, evaluate_parallel)
                };
                (cpu_outcomes, false)
            }
        };
        let snapshot_refresh_interval = Duration::from_millis(SNAPSHOT_REFRESH_MS);
        let snapshot_is_stale = self
            .last_snapshot_sample_at
            .map(|last_sample_time| last_sample_time.elapsed() >= snapshot_refresh_interval)
            .unwrap_or(true);
        let round_preview_sample = if snapshot_is_stale {
            let last_outcome_has_round = batch_outcomes
                .last()
                .and_then(|final_outcome| final_outcome.last_round.as_ref())
                .is_some();
            if last_outcome_has_round {
                None
            } else {
                pending_matchup_slice
                    .last()
                    .cloned()
                    .map(|trailing_matchup| {
                        let preview = evaluate_single_matchup(&trailing_matchup, false);
                        self.last_snapshot_sample_at = Some(Instant::now());
                        preview
                    })
            }
        } else {
            None
        };
        if !executed_on_gpu {
            self.runtime.note_cpu_matches(pending_matchup_slice.len());
        }

        self.last_round = None;
        let final_matchup_index = pending_matchup_slice.len();
        for (ordinal, mut completed_outcome) in batch_outcomes.into_iter().enumerate() {
            if ordinal + 1 == final_matchup_index {
                if let Some(sampled_preview) = round_preview_sample.as_ref() {
                    completed_outcome.last_round = sampled_preview.last_round.clone();
                }
            }
            self.record_completed_outcome(completed_outcome);
        }
        *remaining_step_budget = remaining_step_budget
            .saturating_sub((batch_match_count as u32).saturating_mul(rounds_per_match));
        if self.is_done() {
            self.emit(GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }
    }

    pub(super) fn record_completed_outcome(&mut self, outcome: MatchOutcome) {
        let completed_ordinal = self.match_index.saturating_add(1);
        self.last_round = outcome.last_round.clone();
        let progress_needs_update = self
            .last_progress
            .as_ref()
            .map(|tracked| (tracked.match_index, tracked.round))
            != Some((completed_ordinal, outcome.result.rounds));
        if progress_needs_update {
            self.last_progress = Some(crate::tournament::types::TournamentProgress::build(
                completed_ordinal,
                self.schedule.len().max(1),
                outcome.result.rounds,
                outcome.result.rounds,
                true,
                crate::tournament::session::strategy_log_id(&self.strategies[outcome.result.a_idx]),
                crate::tournament::session::strategy_log_id(&self.strategies[outcome.result.b_idx]),
                outcome.result.a_total,
                outcome.result.b_total,
                outcome.last_round.as_ref(),
                self.runtime.clone(),
            ));
        }
        if outcome.a_crashed {
            self.results.strategies[outcome.result.a_idx].crash_count += 1;
            self.results.strategies[outcome.result.a_idx].crashed = true;
        }
        if outcome.b_crashed {
            self.results.strategies[outcome.result.b_idx].crash_count += 1;
            self.results.strategies[outcome.result.b_idx].crashed = true;
        }
        self.results.apply_outcome(outcome);
        self.match_index += 1;
    }
}
