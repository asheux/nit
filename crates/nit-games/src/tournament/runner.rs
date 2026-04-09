//! Step-driven tournament runner for interactive TUI playback.
//!
//! [`TournamentRunner`] drives the tournament one step at a time via
//! [`step_rounds`](TournamentRunner::step_rounds), allowing the TUI to
//! render progress, match snapshots, and leaderboard updates between ticks.
//! For batch (run-to-completion) execution, see [`super::kernel::TournamentKernel`].

use super::halting::select_halting_turing_machine_strategies;
use super::metal::{
    adjusted_total_for_match, metal_batch_decline_reason,
    try_metal_batch_outcomes_chunked_prepared, try_prepare_metal_batch_for_workload,
};
use super::schedule::SchedulePlan;
use super::session::{
    build_strategy_definitions, play_round_core, run_match_core, strategy_log_id,
    tm_metrics_from_stats,
};
use super::types::{
    run_with_parallelism, MatchHistoryPreview, MatchOutcome, MatchResult, MatchSession,
    MatchSnapshot, Matchup, MetalBatchState, Parallelism, RoundSnapshot, SeedDeriver,
    TournamentAccumulator, TournamentProgress,
};
use crate::config::{NormalizedConfig, StrategySpec};
use crate::events::{EventWriter, GameEvent};
use crate::fast_eval::FastStrategyModel;
use crate::history_log::{HistoryWriter, MatchHistory};
use crate::output::{RunSummary, RuntimeAcceleratorStats, StrategyDefinition, TournamentResults};
use rayon::prelude::*;
use std::time::{Duration, Instant};

/// How often (in wall-clock time) the TUI samples a round-level preview
/// snapshot during batch fast-forward execution.
const SNAPSHOT_REFRESH_MS: u64 = 100;

/// Sentinel value used when a schedule contains zero matches but a progress
/// snapshot is still requested (e.g. during empty-roster dry runs).
const EMPTY_SCHEDULE_SENTINEL: usize = 0;

/// Incremental tournament executor with match-level and round-level stepping,
/// used by the TUI for live playback.
pub struct TournamentRunner {
    config: NormalizedConfig,
    seed: u64,
    schedule: SchedulePlan,
    match_index: usize,
    current: Option<MatchSession>,
    results: TournamentAccumulator,
    strategies: Vec<StrategySpec>,
    definitions: Vec<StrategyDefinition>,
    seed_deriver: SeedDeriver,
    fast_models: Vec<Option<FastStrategyModel>>,
    event_writer: Option<EventWriter>,
    history_writer: Option<HistoryWriter>,
    last_round: Option<RoundSnapshot>,
    last_progress: Option<TournamentProgress>,
    runtime: RuntimeAcceleratorStats,
    metal_batch: MetalBatchState,
    collect_match_history_previews: bool,
    completed_history_previews: Vec<MatchHistoryPreview>,
    last_snapshot_sample_at: Option<Instant>,
}

impl TournamentRunner {
    pub fn new(config: NormalizedConfig) -> Self {
        let mut config = select_halting_turing_machine_strategies(config);
        let seed = config.seed.unwrap_or(0);
        config.seed = Some(seed);
        let schedule = SchedulePlan::new(
            config.strategies.len(),
            config.repetitions,
            config.self_play,
        );
        let seed_deriver = SeedDeriver::new(seed);
        let definitions = build_strategy_definitions(&config.strategies);
        let fast_models = config
            .strategies
            .iter()
            .map(FastStrategyModel::from_spec)
            .collect();
        let results = TournamentAccumulator::new(
            config.strategies.len(),
            config.engine.complexity_cost.enabled,
            config.engine.score_aggregation,
            !matches!(config.engine.mode, crate::config::EngineMode::Batch),
        );
        let strategies = config.strategies.clone();
        let runtime = RuntimeAcceleratorStats::new(config.engine.accelerator);
        Self {
            config,
            seed,
            schedule,
            match_index: 0,
            current: None,
            results,
            strategies,
            definitions,
            seed_deriver,
            fast_models,
            event_writer: None,
            history_writer: None,
            last_round: None,
            last_progress: None,
            runtime,
            metal_batch: MetalBatchState::Uninitialized,
            collect_match_history_previews: true,
            completed_history_previews: Vec::new(),
            last_snapshot_sample_at: None,
        }
    }

    pub fn with_event_writer(mut self, writer: EventWriter) -> Self {
        self.event_writer = Some(writer);
        self
    }

    pub fn with_history_writer(mut self, writer: HistoryWriter) -> Self {
        self.history_writer = Some(writer);
        self
    }

    pub fn with_match_history_previews(mut self, enabled: bool) -> Self {
        self.collect_match_history_previews = enabled;
        if !enabled {
            self.completed_history_previews.clear();
        }
        self
    }

    pub fn drain_match_history_previews(&mut self) -> Vec<MatchHistoryPreview> {
        std::mem::take(&mut self.completed_history_previews)
    }

    pub fn is_done(&self) -> bool {
        self.match_index >= self.schedule.len() && self.current.is_none()
    }

    /// Progress snapshot with cascading fallback: active session, batch cache,
    /// next scheduled matchup, then last saved progress.
    pub fn progress(&self) -> Option<TournamentProgress> {
        if self.schedule.is_empty() {
            return Some(TournamentProgress::build(
                EMPTY_SCHEDULE_SENTINEL,
                EMPTY_SCHEDULE_SENTINEL,
                0,
                self.config.rounds,
                false,
                "-".into(),
                "-".into(),
                0,
                0,
                None,
                self.runtime.clone(),
            ));
        }
        if let Some(active_session) = self.current.as_ref() {
            return self.progress_from_active_session(active_session);
        }
        if let Some(cached_batch_progress) = self.progress_from_batch_cache() {
            return Some(cached_batch_progress);
        }
        if let Some(upcoming_matchup) = self.schedule.matchup(self.match_index) {
            let first_label = strategy_log_id(self.strategies.get(upcoming_matchup.a_idx)?);
            let second_label = strategy_log_id(self.strategies.get(upcoming_matchup.b_idx)?);
            return Some(TournamentProgress::build(
                self.match_index.saturating_add(1),
                self.schedule.len().max(1),
                0,
                self.config.rounds,
                false,
                first_label,
                second_label,
                0,
                0,
                None,
                self.runtime.clone(),
            ));
        }
        self.last_progress.clone()
    }

    fn progress_from_active_session(
        &self,
        active_session: &MatchSession,
    ) -> Option<TournamentProgress> {
        let active_matchup = &active_session.matchup;
        let first_label = strategy_log_id(self.strategies.get(active_matchup.a_idx)?);
        let second_label = strategy_log_id(self.strategies.get(active_matchup.b_idx)?);
        let previous_round_snapshot = if active_session.round > 0 {
            self.last_round.as_ref()
        } else {
            None
        };
        Some(TournamentProgress::build(
            self.match_index.saturating_add(1),
            self.schedule.len().max(1),
            active_session.round,
            active_session.rounds_total,
            false,
            first_label,
            second_label,
            active_session.a_total,
            active_session.b_total,
            previous_round_snapshot,
            self.runtime.clone(),
        ))
    }

    fn progress_from_batch_cache(&self) -> Option<TournamentProgress> {
        if !matches!(self.config.engine.mode, crate::config::EngineMode::Batch) {
            return None;
        }
        let mut cached_snapshot = self.last_progress.clone()?;
        cached_snapshot.runtime = self.runtime.clone();
        Some(cached_snapshot)
    }

    pub fn match_snapshot(&self) -> Option<MatchSnapshot> {
        let active_session = self.current.as_ref()?;
        let active_matchup = &active_session.matchup;
        let first_label = strategy_log_id(self.strategies.get(active_matchup.a_idx)?);
        let second_label = strategy_log_id(self.strategies.get(active_matchup.b_idx)?);
        Some(MatchSnapshot {
            match_index: self.match_index.saturating_add(1),
            total_matches: self.schedule.len().max(1),
            round: active_session.round,
            rounds: active_session.rounds_total,
            a: first_label,
            b: second_label,
            a_score: active_session.a_total,
            b_score: active_session.b_total,
            outcomes: active_session.history_scores.clone(),
            payoffs: active_session.history_payoffs.clone(),
            a_halted: active_session.history_halted_a.clone(),
            b_halted: active_session.history_halted_b.clone(),
        })
    }

    /// Advance by up to `steps` rounds, opening new matches and fast-forwarding
    /// via batch evaluation when eligible.
    pub fn step_rounds(&mut self, steps: u32) {
        if self.schedule.is_empty() {
            return;
        }
        if self.match_index == 0 && self.current.is_none() {
            self.emit(GameEvent::TournamentStart {
                timestamp: EventWriter::timestamp(),
                total_matches: self.schedule.len(),
                rounds: self.config.rounds,
            });
        }
        let mut remaining_step_budget = steps;
        self.try_fast_forward_matches(&mut remaining_step_budget);
        while remaining_step_budget > 0 {
            if self.is_done() {
                break;
            }
            if self.current.is_none() {
                let Some(next_matchup) = self.schedule.matchup(self.match_index) else {
                    break;
                };
                self.open_new_match(next_matchup);
            }

            let Some(mut active_session) = self.current.take() else {
                break;
            };
            let round_snapshot = self.play_round(&mut active_session);
            self.last_round = Some(round_snapshot.clone());
            self.last_progress = Some(TournamentProgress::build(
                self.match_index.saturating_add(1),
                self.schedule.len().max(1),
                active_session.round,
                active_session.rounds_total,
                false,
                strategy_log_id(&self.strategies[active_session.matchup.a_idx]),
                strategy_log_id(&self.strategies[active_session.matchup.b_idx]),
                active_session.a_total,
                active_session.b_total,
                Some(&round_snapshot),
                self.runtime.clone(),
            ));
            if active_session.round >= active_session.rounds_total {
                self.finalize_completed_session(active_session);
            } else {
                self.current = Some(active_session);
            }
            remaining_step_budget = remaining_step_budget.saturating_sub(1);
            self.try_fast_forward_matches(&mut remaining_step_budget);
        }
    }

    fn open_new_match(&mut self, next_matchup: Matchup) {
        let new_session = MatchSession::new(
            next_matchup,
            &self.config,
            &self.strategies,
            &self.seed_deriver,
            true,
            true,
        );
        let player_a_id = self.strategies[new_session.matchup.a_idx].id.clone();
        let player_b_id = self.strategies[new_session.matchup.b_idx].id.clone();
        self.emit(GameEvent::MatchStart {
            timestamp: EventWriter::timestamp(),
            match_id: new_session.matchup.match_id,
            match_index: self.match_index + 1,
            total_matches: self.schedule.len(),
            a: player_a_id.clone(),
            b: player_b_id.clone(),
            repetition: new_session.matchup.repetition + 1,
        });
        self.last_progress = Some(TournamentProgress::build(
            self.match_index.saturating_add(1),
            self.schedule.len().max(1),
            0,
            new_session.rounds_total,
            false,
            player_a_id,
            player_b_id,
            0,
            0,
            None,
            self.runtime.clone(),
        ));
        self.current = Some(new_session);
    }

    fn finalize_completed_session(&mut self, completed_session: MatchSession) {
        if self.collect_match_history_previews {
            self.completed_history_previews.push(MatchHistoryPreview {
                match_index: self.match_index.saturating_add(1),
                total_matches: self.schedule.len().max(1),
                a: strategy_log_id(&self.strategies[completed_session.matchup.a_idx]),
                b: strategy_log_id(&self.strategies[completed_session.matchup.b_idx]),
                rounds_total: completed_session.rounds_total,
                outcomes: completed_session.history_scores.clone(),
            });
        }
        let first_strategy_spec = &self.strategies[completed_session.matchup.a_idx];
        let second_strategy_spec = &self.strategies[completed_session.matchup.b_idx];
        let complexity_cost_config = &self.config.engine.complexity_cost;
        let first_tm_stats = completed_session.a_strategy.tm_stats();
        let second_tm_stats = completed_session.b_strategy.tm_stats();
        let first_adjusted_total = adjusted_total_for_match(
            completed_session.a_total,
            first_strategy_spec,
            completed_session.rounds_total,
            first_tm_stats,
            complexity_cost_config,
        );
        let second_adjusted_total = adjusted_total_for_match(
            completed_session.b_total,
            second_strategy_spec,
            completed_session.rounds_total,
            second_tm_stats,
            complexity_cost_config,
        );
        let match_result = MatchResult {
            a_idx: completed_session.matchup.a_idx,
            b_idx: completed_session.matchup.b_idx,
            rounds: completed_session.rounds_total,
            a_total: completed_session.a_total,
            b_total: completed_session.b_total,
            a_adjusted_total: first_adjusted_total,
            b_adjusted_total: second_adjusted_total,
            repetition: completed_session.matchup.repetition,
            match_id: completed_session.matchup.match_id,
        };
        self.emit(GameEvent::MatchEnd {
            timestamp: EventWriter::timestamp(),
            match_id: completed_session.matchup.match_id,
            match_index: self.match_index + 1,
            a_total: completed_session.a_total,
            b_total: completed_session.b_total,
        });
        self.emit_history(&completed_session);
        self.runtime.note_cpu_matches(1);
        self.record_completed_outcome(MatchOutcome {
            result: match_result,
            a_crashed: completed_session.a_crashed,
            b_crashed: completed_session.b_crashed,
            a_tm_stats: first_tm_stats.cloned(),
            b_tm_stats: second_tm_stats.cloned(),
            last_round: self.last_round.clone(),
        });
        if self.is_done() {
            self.emit(GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }
    }

    pub fn results(&self) -> TournamentResults {
        self.results.finalize(&self.strategies)
    }

    pub fn leaderboard(&self) -> TournamentResults {
        self.results.leaderboard(&self.strategies)
    }

    pub fn definitions(&self) -> &[StrategyDefinition] {
        &self.definitions
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Borrow the normalised configuration driving this tournament.
    pub fn config(&self) -> &NormalizedConfig {
        &self.config
    }

    /// Current runtime accelerator statistics (CPU/GPU match counts).
    pub fn runtime(&self) -> &RuntimeAcceleratorStats {
        &self.runtime
    }

    /// Number of matches that have been fully scored so far.
    pub fn completed_matches(&self) -> usize {
        self.match_index
    }

    /// Total number of matches in the tournament schedule.
    pub fn total_matches(&self) -> usize {
        self.schedule.len()
    }

    /// Consume the runner, flush writers, and produce the final [`RunSummary`].
    ///
    /// Closes event and history log files, builds the result ranking, and
    /// assembles all metadata into a single summary struct for serialisation.
    pub fn finish(mut self, timestamp: String, run_id: String, config_text: String) -> RunSummary {
        let event_log_path = self
            .event_writer
            .take()
            .and_then(|pending_writer| pending_writer.finish().ok())
            .map(|resolved_path| resolved_path.to_string_lossy().to_string());
        let history_log_path = self
            .history_writer
            .take()
            .and_then(|pending_writer| pending_writer.finish().ok())
            .map(|resolved_path| resolved_path.to_string_lossy().to_string());
        let final_results = self.results();
        RunSummary {
            schema_version: crate::output::RUN_SUMMARY_SCHEMA_VERSION,
            timestamp,
            run_id,
            seed: self.seed,
            config_text,
            config: self.config.clone(),
            paths: crate::output::RunPaths {
                summary: None,
                events: event_log_path.clone(),
                history: history_log_path.clone(),
                definitions: None,
                results: None,
                config: None,
                analysis_dir: None,
            },
            strategies: self.definitions.clone(),
            results: final_results,
            event_log: event_log_path,
            history_log: history_log_path,
            runtime: self.runtime.clone(),
            run_dir: None,
        }
    }

    /// Write a game event to the attached event log, if present.
    ///
    /// Round-level events are suppressed when the writer is configured to
    /// exclude per-round granularity.
    fn emit(&mut self, event: GameEvent) {
        let Some(event_log_writer) = self.event_writer.as_mut() else {
            return;
        };
        if matches!(event, GameEvent::Round { .. }) && !event_log_writer.include_rounds() {
            return;
        }
        let _ = event_log_writer.write(&event);
    }

    /// Serialise a completed match session into the history log.
    ///
    /// Optionally includes Turing-machine cycle metadata when enabled in
    /// the history configuration.
    fn emit_history(&mut self, finished_session: &MatchSession) {
        let Some(history_log_writer) = self.history_writer.as_mut() else {
            return;
        };
        let first_label = strategy_log_id(&self.strategies[finished_session.matchup.a_idx]);
        let second_label = strategy_log_id(&self.strategies[finished_session.matchup.b_idx]);
        let should_include_tm_metrics = self.config.history.include_cycle_metadata;
        let first_tm_metrics = if should_include_tm_metrics {
            finished_session
                .a_strategy
                .tm_stats()
                .map(tm_metrics_from_stats)
        } else {
            None
        };
        let second_tm_metrics = if should_include_tm_metrics {
            finished_session
                .b_strategy
                .tm_stats()
                .map(tm_metrics_from_stats)
        } else {
            None
        };
        let history_record = MatchHistory {
            match_id: finished_session.matchup.match_id,
            match_index: self.match_index + 1,
            total_matches: self.schedule.len(),
            a: first_label,
            b: second_label,
            repetition: finished_session.matchup.repetition + 1,
            rounds: finished_session.rounds_total,
            score_idx: finished_session.history_scores.clone(),
            a_score: finished_session.a_total,
            b_score: finished_session.b_total,
            cycle: None,
            a_tm_metrics: first_tm_metrics,
            b_tm_metrics: second_tm_metrics,
        };
        let _ = history_log_writer.write(&history_record);
    }

    /// Check whether the batch fast-forward path is eligible.
    ///
    /// Fast-forwarding requires no active session, fast-eval enabled, zero
    /// noise, and no attached event/history writers or preview collection.
    fn fast_forward_allowed(&self) -> bool {
        self.current.is_none()
            && self.config.engine.fast_eval
            && self.config.noise == 0.0
            && self.event_writer.is_none()
            && self.history_writer.is_none()
            && !self.collect_match_history_previews
    }

    /// Lazily initialise the Metal batch pipeline when the GPU accelerator
    /// has not yet been probed for this tournament.
    ///
    /// After this call, `self.metal_batch` is guaranteed to be either
    /// `Prepared` or `Unavailable` (never `Uninitialized`).
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

    /// Record the reason the Metal batch evaluator declined this workload,
    /// but only when the accelerator configuration permits Metal.
    fn note_metal_decline_reason(&mut self) {
        if !self.config.engine.accelerator.allows_metal() {
            return;
        }
        let decline_reason = metal_batch_decline_reason(&self.config, &self.strategies, 1)
            .unwrap_or_else(|| "Metal batch evaluator declined this workload".into());
        self.runtime.note_metal_fallback_reason(decline_reason);
    }

    /// Attempt to skip ahead by evaluating multiple complete matches in a
    /// single batch, using the Metal GPU pipeline when available or falling
    /// back to parallel CPU evaluation.
    ///
    /// This is the primary fast-path for batch mode: instead of stepping one
    /// round at a time, entire matches are evaluated in bulk.
    fn try_fast_forward_matches(&mut self, remaining_step_budget: &mut u32) {
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
        // Rate-limit the snapshot sample: running a full match round-by-round
        // to obtain a last_round preview is O(rounds) on the CPU.  At high
        // round counts (e.g. 500K) this dominates each tick.  Only recompute
        // when enough wall-clock time has elapsed since the previous sample.
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

    /// Integrate a completed match outcome into the accumulator and advance
    /// the match index.
    ///
    /// Updates the leaderboard progress, marks any crash flags on the
    /// per-strategy stats, and delegates scoring to `TournamentAccumulator`.
    fn record_completed_outcome(&mut self, outcome: MatchOutcome) {
        let completed_ordinal = self.match_index.saturating_add(1);
        self.last_round = outcome.last_round.clone();
        let progress_needs_update = self
            .last_progress
            .as_ref()
            .map(|tracked| (tracked.match_index, tracked.round))
            != Some((completed_ordinal, outcome.result.rounds));
        if progress_needs_update {
            self.last_progress = Some(TournamentProgress::build(
                completed_ordinal,
                self.schedule.len().max(1),
                outcome.result.rounds,
                outcome.result.rounds,
                true,
                strategy_log_id(&self.strategies[outcome.result.a_idx]),
                strategy_log_id(&self.strategies[outcome.result.b_idx]),
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

    /// Execute a single round of the active match session, recording any
    /// strategy crashes and emitting the round-level game event.
    ///
    /// Returns the round snapshot for use in progress updates and
    /// last-round caching.
    fn play_round(&mut self, active_session: &mut MatchSession) -> RoundSnapshot {
        self.runtime.note_cpu_activity();
        let first_player_idx = active_session.matchup.a_idx;
        let second_player_idx = active_session.matchup.b_idx;
        let first_strategy_id = self.strategies[first_player_idx].id.clone();
        let second_strategy_id = self.strategies[second_player_idx].id.clone();

        let round_outcome = play_round_core(active_session, &self.config);

        if round_outcome.a_crash_now {
            active_session.a_crashed = true;
            self.results.strategies[first_player_idx].crash_count += 1;
            self.results.strategies[first_player_idx].crashed = true;
            self.emit(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: first_strategy_id,
                error: "panic in strategy".into(),
            });
        }
        if round_outcome.b_crash_now {
            active_session.b_crashed = true;
            self.results.strategies[second_player_idx].crash_count += 1;
            self.results.strategies[second_player_idx].crashed = true;
            self.emit(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: second_strategy_id,
                error: "panic in strategy".into(),
            });
        }

        self.emit(GameEvent::Round {
            timestamp: EventWriter::timestamp(),
            match_id: active_session.matchup.match_id,
            match_index: self.match_index + 1,
            round: active_session.round,
            a_action: round_outcome.snapshot.a_action.as_char(),
            b_action: round_outcome.snapshot.b_action.as_char(),
            a_halted: round_outcome.snapshot.a_halted,
            b_halted: round_outcome.snapshot.b_halted,
            a_payoff: round_outcome.snapshot.a_payoff,
            b_payoff: round_outcome.snapshot.b_payoff,
        });
        round_outcome.snapshot
    }
}
