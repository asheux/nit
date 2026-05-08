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
        let remaining = self.schedule.len().saturating_sub(self.match_index);
        match try_prepare_metal_batch_for_workload(&self.config, &self.strategies, remaining) {
            Ok(Some(prepared)) => {
                self.runtime.note_metal_policy(
                    prepared.policy.matches_per_batch,
                    prepared.policy.inflight_batches,
                    prepared.policy_source,
                    prepared.policy_cache_key.clone(),
                    prepared.policy_cache_path.clone(),
                );
                self.metal_batch = MetalBatchState::Prepared(prepared);
            }
            Ok(None) => {
                self.note_metal_decline_reason();
                self.metal_batch = MetalBatchState::Unavailable;
            }
            Err(err) => {
                if self.config.engine.accelerator.allows_metal() {
                    self.runtime
                        .note_metal_fallback_reason(format!("Metal backend error: {err}"));
                }
                self.metal_batch = MetalBatchState::Unavailable;
            }
        }
    }

    fn note_metal_decline_reason(&mut self) {
        if !self.config.engine.accelerator.allows_metal() {
            return;
        }
        let reason = metal_batch_decline_reason(&self.config, &self.strategies, 1)
            .unwrap_or_else(|| "Metal batch evaluator declined this workload".into());
        self.runtime.note_metal_fallback_reason(reason);
    }

    pub(super) fn try_fast_forward_matches(&mut self, remaining_step_budget: &mut u32) {
        if !self.fast_forward_allowed() {
            return;
        }
        let rounds_per_match = self.config.rounds.max(1);
        let affordable = (*remaining_step_budget / rounds_per_match) as usize;
        if affordable == 0 {
            return;
        }
        let schedulable = self.schedule.len().saturating_sub(self.match_index);
        let batch_count = affordable.min(schedulable);
        if batch_count == 0 {
            return;
        }

        let matchups = self.schedule.matchups(self.match_index, batch_count);
        if matchups.is_empty() {
            return;
        }
        self.ensure_metal_batch();
        let scheduled_total = self.schedule.len();
        let (outcomes, executed_on_gpu) = self.evaluate_batch(&matchups, scheduled_total);
        let preview = self.maybe_collect_round_preview(&matchups, &outcomes, scheduled_total);
        if !executed_on_gpu {
            self.runtime.note_cpu_matches(matchups.len());
        }

        self.last_round = None;
        let final_idx = matchups.len();
        for (ordinal, mut outcome) in outcomes.into_iter().enumerate() {
            if ordinal + 1 == final_idx {
                if let Some(sample) = preview.as_ref() {
                    outcome.last_round = sample.last_round.clone();
                }
            }
            self.record_completed_outcome(outcome);
        }
        *remaining_step_budget = remaining_step_budget
            .saturating_sub((batch_count as u32).saturating_mul(rounds_per_match));
        if self.is_done() {
            self.emit(GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }
    }

    fn evaluate_batch(
        &mut self,
        matchups: &[Matchup],
        scheduled_total: usize,
    ) -> (Vec<MatchOutcome>, bool) {
        let gpu_result = match &self.metal_batch {
            MetalBatchState::Prepared(prepared) => try_metal_batch_outcomes_chunked_prepared(
                &self.config,
                &self.strategies,
                prepared,
                matchups,
            ),
            MetalBatchState::Uninitialized | MetalBatchState::Unavailable => Ok(None),
        };
        match gpu_result {
            Ok(Some((gpu_outcomes, batches))) => {
                self.runtime.note_metal_batches(batches, matchups.len());
                (gpu_outcomes, true)
            }
            other => {
                if let Err(err) = other {
                    if !matchups.is_empty() && self.config.engine.accelerator.allows_metal() {
                        self.runtime
                            .note_metal_fallback_reason(format!("Metal backend error: {err}"));
                        self.metal_batch = MetalBatchState::Unavailable;
                    }
                }
                (
                    self.evaluate_batch_cpu(matchups, scheduled_total, true),
                    false,
                )
            }
        }
    }

    fn evaluate_batch_cpu(
        &self,
        matchups: &[Matchup],
        scheduled_total: usize,
        fast_eval_enabled: bool,
    ) -> Vec<MatchOutcome> {
        // Borrow individual fields (not `&self`) so the closure stays Send —
        // `MatchSession` inside `self.current` is not Sync.
        let config = &self.config;
        let strategies = self.strategies.as_slice();
        let seed_deriver = &self.seed_deriver;
        let fast_models = self.fast_models.as_slice();
        let evaluate_one = move |matchup: &Matchup| {
            let mut discard_event = |_event: GameEvent| {};
            let mut discard_history = |_record: MatchHistory| {};
            run_match_core(
                matchup,
                config,
                strategies,
                seed_deriver,
                Some(fast_models),
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
        let parallelism = Parallelism::from_config(&self.config.engine.parallelism);
        if matches!(parallelism, Parallelism::Off) {
            matchups.iter().map(evaluate_one).collect()
        } else {
            run_with_parallelism(parallelism, || {
                matchups.par_iter().map(evaluate_one).collect()
            })
        }
    }

    fn maybe_collect_round_preview(
        &mut self,
        matchups: &[Matchup],
        outcomes: &[MatchOutcome],
        scheduled_total: usize,
    ) -> Option<MatchOutcome> {
        let snapshot_refresh = Duration::from_millis(SNAPSHOT_REFRESH_MS);
        let stale = self
            .last_snapshot_sample_at
            .map(|t| t.elapsed() >= snapshot_refresh)
            .unwrap_or(true);
        if !stale {
            return None;
        }
        let last_has_round = outcomes
            .last()
            .and_then(|o| o.last_round.as_ref())
            .is_some();
        if last_has_round {
            return None;
        }
        let trailing = matchups.last().cloned()?;
        let preview = self
            .evaluate_batch_cpu(std::slice::from_ref(&trailing), scheduled_total, false)
            .into_iter()
            .next()?;
        self.last_snapshot_sample_at = Some(Instant::now());
        Some(preview)
    }

    pub(super) fn record_completed_outcome(&mut self, outcome: MatchOutcome) {
        let ordinal = self.match_index.saturating_add(1);
        self.last_round = outcome.last_round.clone();
        let progress_stale = self
            .last_progress
            .as_ref()
            .map(|tracked| (tracked.match_index, tracked.round))
            != Some((ordinal, outcome.result.rounds));
        if progress_stale {
            self.last_progress = Some(crate::tournament::types::TournamentProgress::build(
                ordinal,
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
