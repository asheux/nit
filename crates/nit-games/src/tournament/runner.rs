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

/// Incremental tournament executor with match-level and round-level stepping.
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
        let definitions = build_strategy_definitions(&config.strategies, &seed_deriver);
        let fast_models = config
            .strategies
            .iter()
            .map(FastStrategyModel::from_spec)
            .collect();
        let use_adjusted = config.engine.complexity_cost.enabled;
        let results = TournamentAccumulator::new(
            config.strategies.len(),
            use_adjusted,
            config.engine.score_aggregation,
            !matches!(config.engine.mode, crate::config::EngineMode::Batch),
        );
        Self {
            config: config.clone(),
            seed,
            schedule,
            match_index: 0,
            current: None,
            results,
            strategies: config.strategies.clone(),
            definitions,
            seed_deriver,
            fast_models,
            event_writer: None,
            history_writer: None,
            last_round: None,
            last_progress: None,
            runtime: RuntimeAcceleratorStats::new(config.engine.accelerator),
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

    /// Current progress snapshot for the TUI status display.
    pub fn progress(&self) -> Option<TournamentProgress> {
        if self.schedule.is_empty() {
            return Some(TournamentProgress::build(
                0, 0, 0, self.config.rounds, false,
                "-".into(), "-".into(), 0, 0,
                None, self.runtime.clone(),
            ));
        }
        if let Some(current) = self.current.as_ref() {
            let matchup = &current.matchup;
            let a = strategy_log_id(self.strategies.get(matchup.a_idx)?);
            let b = strategy_log_id(self.strategies.get(matchup.b_idx)?);
            let last_round = if current.round > 0 {
                self.last_round.as_ref()
            } else {
                None
            };
            return Some(TournamentProgress::build(
                self.match_index.saturating_add(1),
                self.schedule.len().max(1),
                current.round, current.rounds_total, false,
                a, b, current.a_total, current.b_total,
                last_round, self.runtime.clone(),
            ));
        }
        if matches!(self.config.engine.mode, crate::config::EngineMode::Batch) {
            if let Some(mut progress) = self.last_progress.clone() {
                progress.runtime = self.runtime.clone();
                return Some(progress);
            }
        }
        if let Some(next_match) = self.schedule.matchup(self.match_index) {
            let a = strategy_log_id(self.strategies.get(next_match.a_idx)?);
            let b = strategy_log_id(self.strategies.get(next_match.b_idx)?);
            return Some(TournamentProgress::build(
                self.match_index.saturating_add(1),
                self.schedule.len().max(1),
                0, self.config.rounds, false,
                a, b, 0, 0,
                None, self.runtime.clone(),
            ));
        }
        self.last_progress.clone()
    }

    pub fn match_snapshot(&self) -> Option<MatchSnapshot> {
        let current = self.current.as_ref()?;
        let matchup = &current.matchup;
        let a = strategy_log_id(self.strategies.get(matchup.a_idx)?);
        let b = strategy_log_id(self.strategies.get(matchup.b_idx)?);
        Some(MatchSnapshot {
            match_index: self.match_index.saturating_add(1),
            total_matches: self.schedule.len().max(1),
            round: current.round,
            rounds: current.rounds_total,
            a,
            b,
            a_score: current.a_total,
            b_score: current.b_total,
            outcomes: current.history_scores.clone(),
            payoffs: current.history_payoffs.clone(),
            a_halted: current.history_halted_a.clone(),
            b_halted: current.history_halted_b.clone(),
        })
    }

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
        let mut remaining_steps = steps;
        self.try_fast_forward_matches(&mut remaining_steps);
        while remaining_steps > 0 {
            if self.is_done() {
                break;
            }
            if self.current.is_none() {
                if let Some(matchup) = self.schedule.matchup(self.match_index) {
                    let session = MatchSession::new(
                        matchup,
                        &self.config,
                        &self.strategies,
                        &self.seed_deriver,
                        true,
                        true,
                    );
                    self.emit(GameEvent::MatchStart {
                        timestamp: EventWriter::timestamp(),
                        match_id: session.matchup.match_id,
                        match_index: self.match_index + 1,
                        total_matches: self.schedule.len(),
                        a: self.strategies[session.matchup.a_idx].id.clone(),
                        b: self.strategies[session.matchup.b_idx].id.clone(),
                        repetition: session.matchup.repetition + 1,
                    });
                    self.last_progress = Some(TournamentProgress::build(
                        self.match_index.saturating_add(1),
                        self.schedule.len().max(1),
                        0, session.rounds_total, false,
                        self.strategies[session.matchup.a_idx].id.clone(),
                        self.strategies[session.matchup.b_idx].id.clone(),
                        0, 0, None, self.runtime.clone(),
                    ));
                    self.current = Some(session);
                } else {
                    break;
                }
            }

            if let Some(mut session) = self.current.take() {
                let snapshot = self.play_round(&mut session);
                self.last_round = Some(snapshot.clone());
                self.last_progress = Some(TournamentProgress::build(
                    self.match_index.saturating_add(1),
                    self.schedule.len().max(1),
                    session.round, session.rounds_total, false,
                    strategy_log_id(&self.strategies[session.matchup.a_idx]),
                    strategy_log_id(&self.strategies[session.matchup.b_idx]),
                    session.a_total, session.b_total,
                    Some(&snapshot), self.runtime.clone(),
                ));
                if session.round >= session.rounds_total {
                    if self.collect_match_history_previews {
                        self.completed_history_previews.push(MatchHistoryPreview {
                            match_index: self.match_index.saturating_add(1),
                            total_matches: self.schedule.len().max(1),
                            a: strategy_log_id(&self.strategies[session.matchup.a_idx]),
                            b: strategy_log_id(&self.strategies[session.matchup.b_idx]),
                            rounds_total: session.rounds_total,
                            outcomes: session.history_scores.clone(),
                        });
                    }
                    let a_spec = &self.strategies[session.matchup.a_idx];
                    let b_spec = &self.strategies[session.matchup.b_idx];
                    let cost = &self.config.engine.complexity_cost;
                    let a_tm_stats = session.a_strategy.tm_stats();
                    let b_tm_stats = session.b_strategy.tm_stats();
                    let a_adjusted_total = adjusted_total_for_match(
                        session.a_total,
                        a_spec,
                        session.rounds_total,
                        a_tm_stats,
                        cost,
                    );
                    let b_adjusted_total = adjusted_total_for_match(
                        session.b_total,
                        b_spec,
                        session.rounds_total,
                        b_tm_stats,
                        cost,
                    );
                    let result = MatchResult {
                        a_idx: session.matchup.a_idx,
                        b_idx: session.matchup.b_idx,
                        rounds: session.rounds_total,
                        a_total: session.a_total,
                        b_total: session.b_total,
                        a_adjusted_total,
                        b_adjusted_total,
                        repetition: session.matchup.repetition,
                        match_id: session.matchup.match_id,
                    };
                    self.emit(GameEvent::MatchEnd {
                        timestamp: EventWriter::timestamp(),
                        match_id: session.matchup.match_id,
                        match_index: self.match_index + 1,
                        a_total: session.a_total,
                        b_total: session.b_total,
                    });
                    self.emit_history(&session);
                    self.runtime.note_cpu_matches(1);
                    self.record_completed_outcome(MatchOutcome {
                        result,
                        a_crashed: session.a_crashed,
                        b_crashed: session.b_crashed,
                        a_tm_stats: a_tm_stats.cloned(),
                        b_tm_stats: b_tm_stats.cloned(),
                        last_round: self.last_round.clone(),
                    });
                    if self.is_done() {
                        self.emit(GameEvent::TournamentEnd {
                            timestamp: EventWriter::timestamp(),
                        });
                    }
                } else {
                    self.current = Some(session);
                }
            }
            remaining_steps = remaining_steps.saturating_sub(1);
            self.try_fast_forward_matches(&mut remaining_steps);
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

    pub fn config(&self) -> &NormalizedConfig {
        &self.config
    }

    pub fn runtime(&self) -> &RuntimeAcceleratorStats {
        &self.runtime
    }

    pub fn completed_matches(&self) -> usize {
        self.match_index
    }

    pub fn total_matches(&self) -> usize {
        self.schedule.len()
    }

    pub fn finish(mut self, timestamp: String, run_id: String, config_text: String) -> RunSummary {
        let event_log = self
            .event_writer
            .take()
            .and_then(|writer| writer.finish().ok())
            .map(|p| p.to_string_lossy().to_string());
        let history_log = self
            .history_writer
            .take()
            .and_then(|writer| writer.finish().ok())
            .map(|p| p.to_string_lossy().to_string());
        let results = self.results();
        RunSummary {
            schema_version: crate::output::RUN_SUMMARY_SCHEMA_VERSION,
            timestamp,
            run_id,
            seed: self.seed,
            config_text,
            config: self.config.clone(),
            paths: crate::output::RunPaths {
                summary: None,
                events: event_log.clone(),
                history: history_log.clone(),
                definitions: None,
                results: None,
                config: None,
                analysis_dir: None,
            },
            strategies: self.definitions.clone(),
            results,
            event_log,
            history_log,
            runtime: self.runtime.clone(),
            run_dir: None,
        }
    }

    fn emit(&mut self, event: GameEvent) {
        if let Some(writer) = self.event_writer.as_mut() {
            if matches!(event, GameEvent::Round { .. }) && !writer.include_rounds() {
                return;
            }
            let _ = writer.write(&event);
        }
    }

    fn emit_history(&mut self, session: &MatchSession) {
        let Some(writer) = self.history_writer.as_mut() else {
            return;
        };
        let a = strategy_log_id(&self.strategies[session.matchup.a_idx]);
        let b = strategy_log_id(&self.strategies[session.matchup.b_idx]);
        let include_tm_metrics = self.config.history.include_cycle_metadata;
        let a_tm_metrics = if include_tm_metrics {
            session.a_strategy.tm_stats().map(tm_metrics_from_stats)
        } else {
            None
        };
        let b_tm_metrics = if include_tm_metrics {
            session.b_strategy.tm_stats().map(tm_metrics_from_stats)
        } else {
            None
        };
        let record = MatchHistory {
            match_id: session.matchup.match_id,
            match_index: self.match_index + 1,
            total_matches: self.schedule.len(),
            a,
            b,
            repetition: session.matchup.repetition + 1,
            rounds: session.rounds_total,
            score_idx: session.history_scores.clone(),
            a_score: session.a_total,
            b_score: session.b_total,
            cycle: None,
            a_tm_metrics,
            b_tm_metrics,
        };
        let _ = writer.write(&record);
    }

    fn fast_forward_allowed(&self) -> bool {
        self.current.is_none()
            && self.config.engine.fast_eval
            && self.config.noise == 0.0
            && self.event_writer.is_none()
            && self.history_writer.is_none()
            && !self.collect_match_history_previews
    }

    fn ensure_metal_batch(&mut self) {
        if matches!(self.metal_batch, MetalBatchState::Uninitialized) {
            let remaining_matches = self.schedule.len().saturating_sub(self.match_index);
            match try_prepare_metal_batch_for_workload(
                &self.config,
                &self.strategies,
                remaining_matches,
            ) {
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
                    if self.config.engine.accelerator.allows_metal() {
                        self.runtime.note_metal_fallback_reason(
                            metal_batch_decline_reason(&self.config, &self.strategies, 1)
                                .unwrap_or_else(|| {
                                    "Metal batch evaluator declined this workload".into()
                                }),
                        );
                    }
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
    }

    fn try_fast_forward_matches(&mut self, remaining_steps: &mut u32) {
        if !self.fast_forward_allowed() {
            return;
        }
        let rounds_per_match = self.config.rounds.max(1);
        let match_budget = (*remaining_steps / rounds_per_match) as usize;
        if match_budget == 0 {
            return;
        }
        let available = self.schedule.len().saturating_sub(self.match_index);
        let matches_to_run = match_budget.min(available);
        if matches_to_run == 0 {
            return;
        }

        let total_matches = self.schedule.len();
        let matchups = self.schedule.matchups(self.match_index, matches_to_run);
        if matchups.is_empty() {
            return;
        }
        self.ensure_metal_batch();
        let config = &self.config;
        let strategies = &self.strategies;
        let seed_deriver = &self.seed_deriver;
        let fast_models = &self.fast_models;
        let run_matchup = |matchup: &Matchup, fast_eval_allowed: bool| {
            let mut emit_event = |_event: GameEvent| {};
            let mut emit_history = |_record: MatchHistory| {};
            run_match_core(
                matchup,
                config,
                strategies,
                seed_deriver,
                Some(fast_models),
                fast_eval_allowed,
                total_matches,
                false,
                false,
                &mut emit_event,
                false,
                &mut emit_history,
                false,
            )
        };
        let fast_forward_matchups = matchups.as_slice();
        let run_parallel = || {
            fast_forward_matchups
                .par_iter()
                .map(|matchup| run_matchup(matchup, true))
                .collect::<Vec<_>>()
        };
        let metal_result = match &self.metal_batch {
            MetalBatchState::Prepared(prepared) => try_metal_batch_outcomes_chunked_prepared(
                config,
                strategies,
                prepared,
                fast_forward_matchups,
            ),
            MetalBatchState::Uninitialized | MetalBatchState::Unavailable => Ok(None),
        };
        let (outcomes, gpu_used) = match metal_result {
            Ok(Some((gpu_outcomes, metal_batches))) => {
                self.runtime
                    .note_metal_batches(metal_batches, fast_forward_matchups.len());
                (gpu_outcomes, true)
            }
            Ok(None) => {
                let par = Parallelism::from_config(&self.config.engine.parallelism);
                let outcomes = if matches!(par, Parallelism::Off) {
                    fast_forward_matchups
                        .iter()
                        .map(|matchup| run_matchup(matchup, true))
                        .collect()
                } else {
                    run_with_parallelism(par, run_parallel)
                };
                (outcomes, false)
            }
            Err(err) => {
                if !fast_forward_matchups.is_empty()
                    && self.config.engine.accelerator.allows_metal()
                {
                    self.runtime
                        .note_metal_fallback_reason(format!("Metal backend error: {err}"));
                    self.metal_batch = MetalBatchState::Unavailable;
                }
                let par = Parallelism::from_config(&self.config.engine.parallelism);
                let outcomes = if matches!(par, Parallelism::Off) {
                    fast_forward_matchups
                        .iter()
                        .map(|matchup| run_matchup(matchup, true))
                        .collect()
                } else {
                    run_with_parallelism(par, run_parallel)
                };
                (outcomes, false)
            }
        };
        // Rate-limit the snapshot sample: running a full match round-by-round
        // to obtain a last_round preview is O(rounds) on the CPU.  At high
        // round counts (e.g. 500K) this dominates each tick.  Only recompute
        // when enough time has passed since the previous sample.
        let snapshot_interval = Duration::from_millis(100);
        let need_snapshot = self
            .last_snapshot_sample_at
            .map(|t| t.elapsed() >= snapshot_interval)
            .unwrap_or(true);
        let snapshot_sample = if need_snapshot {
            let sample = outcomes
                .last()
                .and_then(|outcome| outcome.last_round.as_ref())
                .is_none()
                .then(|| fast_forward_matchups.last().cloned())
                .flatten()
                .map(|matchup| run_matchup(&matchup, false));
            if sample.is_some() {
                self.last_snapshot_sample_at = Some(Instant::now());
            }
            sample
        } else {
            None
        };
        if !gpu_used {
            self.runtime.note_cpu_matches(fast_forward_matchups.len());
        }

        self.last_round = None;
        for (idx, mut outcome) in outcomes.into_iter().enumerate() {
            if idx + 1 == fast_forward_matchups.len() {
                if let Some(sample) = snapshot_sample.as_ref() {
                    outcome.last_round = sample.last_round.clone();
                }
            }
            self.record_completed_outcome(outcome);
        }
        *remaining_steps = remaining_steps
            .saturating_sub((matches_to_run as u32).saturating_mul(rounds_per_match));
        if self.is_done() {
            self.emit(GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }
    }

    fn record_completed_outcome(&mut self, outcome: MatchOutcome) {
        let MatchOutcome {
            result,
            a_crashed,
            b_crashed,
            a_tm_stats,
            b_tm_stats,
            last_round,
        } = outcome;
        let completed_match = self.match_index.saturating_add(1);
        self.last_round = last_round.clone();
        if self
            .last_progress
            .as_ref()
            .map(|progress| (progress.match_index, progress.round))
            != Some((completed_match, result.rounds))
        {
            self.last_progress = Some(TournamentProgress::build(
                completed_match,
                self.schedule.len().max(1),
                result.rounds, result.rounds, true,
                strategy_log_id(&self.strategies[result.a_idx]),
                strategy_log_id(&self.strategies[result.b_idx]),
                result.a_total, result.b_total,
                last_round.as_ref(), self.runtime.clone(),
            ));
        }
        if a_crashed {
            self.results.strategies[result.a_idx].crash_count += 1;
            self.results.strategies[result.a_idx].crashed = true;
        }
        if b_crashed {
            self.results.strategies[result.b_idx].crash_count += 1;
            self.results.strategies[result.b_idx].crashed = true;
        }
        self.results
            .apply_match(result, a_crashed, b_crashed, a_tm_stats, b_tm_stats);
        self.match_index += 1;
    }

    fn play_round(&mut self, session: &mut MatchSession) -> RoundSnapshot {
        self.runtime.note_cpu_activity();
        let a_idx = session.matchup.a_idx;
        let b_idx = session.matchup.b_idx;
        let a_id = self.strategies[a_idx].id.clone();
        let b_id = self.strategies[b_idx].id.clone();

        let outcome = play_round_core(session, &self.config);

        if outcome.a_crash_now {
            session.a_crashed = true;
            self.results.strategies[a_idx].crash_count += 1;
            self.results.strategies[a_idx].crashed = true;
            self.emit(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: a_id,
                error: "panic in strategy".into(),
            });
        }
        if outcome.b_crash_now {
            session.b_crashed = true;
            self.results.strategies[b_idx].crash_count += 1;
            self.results.strategies[b_idx].crashed = true;
            self.emit(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: b_id,
                error: "panic in strategy".into(),
            });
        }

        self.emit(GameEvent::Round {
            timestamp: EventWriter::timestamp(),
            match_id: session.matchup.match_id,
            match_index: self.match_index + 1,
            round: session.round,
            a_action: outcome.snapshot.a_action.as_char(),
            b_action: outcome.snapshot.b_action.as_char(),
            a_halted: outcome.snapshot.a_halted,
            b_halted: outcome.snapshot.b_halted,
            a_payoff: outcome.snapshot.a_payoff,
            b_payoff: outcome.snapshot.b_payoff,
        });
        outcome.snapshot
    }
}
