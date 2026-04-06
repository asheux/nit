//! Batch tournament kernel — runs all matches to completion in a single call.
//!
//! [`TournamentKernel`] is the simpler of the two tournament drivers. It runs
//! the entire schedule without yielding, making it suitable for CLI batch mode
//! and petri-dish evaluations. For TUI step-by-step playback, see
//! [`super::runner::TournamentRunner`].

use super::halting::select_halting_turing_machine_strategies;
use super::metal::{
    metal_batch_decline_reason, try_metal_batch_outcomes_chunked_prepared,
    try_prepare_metal_batch_for_workload,
};
use super::schedule::SchedulePlan;
use super::session::{build_strategy_definitions, run_match_core};
use super::types::{run_with_parallelism, Parallelism, SeedDeriver, TournamentAccumulator};
use crate::config::{AcceleratorMode, NormalizedConfig};
use crate::events::{EventWriter, GameEvent};
use crate::fast_eval::FastStrategyModel;
use crate::history_log::{HistoryWriter, MatchHistory};
use crate::output::{RuntimeAcceleratorStats, StrategyDefinition, TournamentResults};
use rayon::prelude::*;
use std::sync::mpsc::Sender;

/// Selects between sequential and parallel execution for the kernel.
///
/// `Sequential` passes mutable writers directly, producing ordered output.
/// `Parallel` uses channels so match results arrive in nondeterministic order;
/// callers should use `match_id` / `match_index` fields to reconstruct ordering.
pub enum KernelRunMode<'a> {
    Sequential {
        event_writer: Option<&'a mut EventWriter>,
        history_writer: Option<&'a mut HistoryWriter>,
    },
    Parallel {
        parallelism: Parallelism,
        // Logs are written via channels; NDJSON line order is nondeterministic.
        // Use match_id/match_index fields to reconstruct ordering.
        event_sender: Option<Sender<GameEvent>>,
        include_rounds: bool,
        history_sender: Option<Sender<MatchHistory>>,
    },
}

/// Batch tournament executor: builds the schedule, runs all matches, returns results.
pub struct TournamentKernel {
    config: NormalizedConfig,
    seed: u64,
    schedule: SchedulePlan,
    definitions: Vec<StrategyDefinition>,
    seed_deriver: SeedDeriver,
    fast_models: Vec<Option<FastStrategyModel>>,
}

impl TournamentKernel {
    /// Build a kernel from a normalised configuration.
    ///
    /// Applies the TM halting filter, derives seeds, pre-computes fast-eval
    /// models, and builds the full match schedule.
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
        Self {
            config,
            seed,
            schedule,
            definitions,
            seed_deriver,
            fast_models,
        }
    }

    /// Run the full tournament and return the final results (discarding runtime stats).
    pub fn run(&self, mode: KernelRunMode<'_>) -> TournamentResults {
        self.run_with_runtime(mode).0
    }

    /// Run the full tournament, returning both results and runtime accelerator stats.
    ///
    /// Dispatches to `run_sequential` or `run_parallel` based on the [`KernelRunMode`].
    pub fn run_with_runtime(
        &self,
        mode: KernelRunMode<'_>,
    ) -> (TournamentResults, RuntimeAcceleratorStats) {
        match mode {
            KernelRunMode::Sequential {
                event_writer,
                history_writer,
            } => self.run_sequential(event_writer, history_writer),
            KernelRunMode::Parallel {
                parallelism,
                event_sender,
                include_rounds,
                history_sender,
            } => self.run_parallel(parallelism, event_sender, include_rounds, history_sender),
        }
    }

    /// The serializable strategy definitions for this tournament.
    pub fn definitions(&self) -> &[StrategyDefinition] {
        &self.definitions
    }

    /// The deterministic seed used for this tournament run.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The normalised configuration (after TM halting filter) used for this run.
    pub fn config(&self) -> &NormalizedConfig {
        &self.config
    }

    /// Total number of matches in the tournament schedule.
    pub fn total_matches(&self) -> usize {
        self.schedule.len()
    }

    fn run_sequential(
        &self,
        mut event_writer: Option<&mut EventWriter>,
        mut history_writer: Option<&mut HistoryWriter>,
    ) -> (TournamentResults, RuntimeAcceleratorStats) {
        let total_matches = self.schedule.len();
        let mut runtime = RuntimeAcceleratorStats::new(self.config.engine.accelerator);
        let mut results = TournamentAccumulator::new(
            self.config.strategies.len(),
            self.config.engine.complexity_cost.enabled,
            self.config.engine.score_aggregation,
            !matches!(self.config.engine.mode, crate::config::EngineMode::Batch),
        );
        if let Some(writer) = event_writer.as_mut() {
            let _ = writer.write(&GameEvent::TournamentStart {
                timestamp: EventWriter::timestamp(),
                total_matches,
                rounds: self.config.rounds,
            });
        }

        let include_rounds = event_writer
            .as_ref()
            .map(|writer| writer.include_rounds())
            .unwrap_or(false);
        let log_events = event_writer.is_some();
        let log_history = history_writer.is_some();

        let fast_eval_allowed = self.config.engine.fast_eval
            && self.config.noise == 0.0
            && !(log_events && include_rounds);

        if fast_eval_allowed
            && !matches!(self.config.engine.accelerator, AcceleratorMode::Cpu)
            && !log_events
            && !log_history
            && !self.schedule.is_empty()
        {
            match try_prepare_metal_batch_for_workload(
                &self.config,
                &self.config.strategies,
                self.schedule.len(),
            ) {
                Ok(Some(prepared)) => {
                    runtime.note_metal_policy(
                        prepared.policy.matches_per_batch,
                        prepared.policy.inflight_batches,
                        prepared.policy_source,
                        prepared.policy_cache_key.clone(),
                        prepared.policy_cache_path.clone(),
                    );
                    let matchups = self.schedule.matchups(0, self.schedule.len());
                    let (outcomes, batches) = try_metal_batch_outcomes_chunked_prepared(
                        &self.config,
                        &self.config.strategies,
                        &prepared,
                        &matchups,
                    )
                    .expect("metal batch support should remain stable across chunks")
                    .expect("metal batch support should remain stable across chunks");
                    for outcome in outcomes {
                        results.apply_match(
                            outcome.result,
                            outcome.a_crashed,
                            outcome.b_crashed,
                            outcome.a_tm_stats,
                            outcome.b_tm_stats,
                        );
                    }
                    runtime.note_metal_batches(batches, self.schedule.len());
                    if let Some(writer) = event_writer.as_mut() {
                        let _ = writer.write(&GameEvent::TournamentEnd {
                            timestamp: EventWriter::timestamp(),
                        });
                    }
                    return (results.finalize(&self.config.strategies), runtime);
                }
                Ok(None) => runtime.note_metal_fallback_reason(
                    metal_batch_decline_reason(&self.config, &self.config.strategies, 1)
                        .unwrap_or_else(|| "Metal batch evaluator declined the probe".into()),
                ),
                Err(err) => {
                    runtime.note_metal_fallback_reason(format!("Metal backend error: {err}"))
                }
            }
        }

        for match_id in 0..self.schedule.len() {
            let matchup = self
                .schedule
                .matchup(match_id)
                .expect("matchup should exist for in-range id");
            let mut emit_event = |event: GameEvent| {
                if let Some(writer) = event_writer.as_mut() {
                    if matches!(event, GameEvent::Round { .. }) && !include_rounds {
                        return;
                    }
                    let _ = writer.write(&event);
                }
            };
            let mut emit_history = |record: MatchHistory| {
                if let Some(writer) = history_writer.as_mut() {
                    let _ = writer.write(&record);
                }
            };
            let outcome = run_match_core(
                &matchup,
                &self.config,
                &self.config.strategies,
                &self.seed_deriver,
                Some(&self.fast_models),
                fast_eval_allowed,
                total_matches,
                log_events,
                include_rounds,
                &mut emit_event,
                log_history,
                &mut emit_history,
                false,
            );
            results.apply_match(
                outcome.result,
                outcome.a_crashed,
                outcome.b_crashed,
                outcome.a_tm_stats,
                outcome.b_tm_stats,
            );
        }
        runtime.note_cpu_matches(self.schedule.len());

        if let Some(writer) = event_writer.as_mut() {
            let _ = writer.write(&GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }
        (results.finalize(&self.config.strategies), runtime)
    }

    fn run_parallel(
        &self,
        parallelism: Parallelism,
        event_sender: Option<Sender<GameEvent>>,
        include_rounds: bool,
        history_sender: Option<Sender<MatchHistory>>,
    ) -> (TournamentResults, RuntimeAcceleratorStats) {
        let total_matches = self.schedule.len();
        let mut runtime = RuntimeAcceleratorStats::new(self.config.engine.accelerator);
        if let Some(sender) = event_sender.as_ref() {
            let _ = sender.send(GameEvent::TournamentStart {
                timestamp: EventWriter::timestamp(),
                total_matches,
                rounds: self.config.rounds,
            });
        }

        let log_events = event_sender.is_some();
        let log_history = history_sender.is_some();
        let event_sender_for_run = event_sender.clone();
        let history_sender_for_run = history_sender.clone();

        let fast_eval_allowed = self.config.engine.fast_eval
            && self.config.noise == 0.0
            && !(log_events && include_rounds);

        if fast_eval_allowed
            && !matches!(self.config.engine.accelerator, AcceleratorMode::Cpu)
            && !log_events
            && !log_history
            && !self.schedule.is_empty()
        {
            match try_prepare_metal_batch_for_workload(
                &self.config,
                &self.config.strategies,
                self.schedule.len(),
            ) {
                Ok(Some(prepared)) => {
                    runtime.note_metal_policy(
                        prepared.policy.matches_per_batch,
                        prepared.policy.inflight_batches,
                        prepared.policy_source,
                        prepared.policy_cache_key.clone(),
                        prepared.policy_cache_path.clone(),
                    );
                    let matchups = self.schedule.matchups(0, self.schedule.len());
                    let (all_outcomes, batches) = try_metal_batch_outcomes_chunked_prepared(
                        &self.config,
                        &self.config.strategies,
                        &prepared,
                        &matchups,
                    )
                    .expect("metal batch support should remain stable across chunks")
                    .expect("metal batch support should remain stable across chunks");
                    runtime.note_metal_batches(batches, self.schedule.len());
                    if let Some(sender) = event_sender.as_ref() {
                        let _ = sender.send(GameEvent::TournamentEnd {
                            timestamp: EventWriter::timestamp(),
                        });
                    }
                    let mut results = TournamentAccumulator::new(
                        self.config.strategies.len(),
                        self.config.engine.complexity_cost.enabled,
                        self.config.engine.score_aggregation,
                        !matches!(self.config.engine.mode, crate::config::EngineMode::Batch),
                    );
                    for outcome in all_outcomes {
                        results.apply_match(
                            outcome.result,
                            outcome.a_crashed,
                            outcome.b_crashed,
                            outcome.a_tm_stats,
                            outcome.b_tm_stats,
                        );
                    }
                    return (results.finalize(&self.config.strategies), runtime);
                }
                Ok(None) => runtime.note_metal_fallback_reason(
                    metal_batch_decline_reason(&self.config, &self.config.strategies, 1)
                        .unwrap_or_else(|| "Metal batch evaluator declined the probe".into()),
                ),
                Err(err) => {
                    runtime.note_metal_fallback_reason(format!("Metal backend error: {err}"))
                }
            }
        }

        let run = || {
            (0..self.schedule.len())
                .into_par_iter()
                .map(move |match_id| {
                    let matchup = self
                        .schedule
                        .matchup(match_id)
                        .expect("matchup should exist for in-range id");
                    let event_tx = event_sender_for_run.clone();
                    let history_tx = history_sender_for_run.clone();
                    let mut emit_event = move |event: GameEvent| {
                        if let Some(sender) = event_tx.as_ref() {
                            let _ = sender.send(event);
                        }
                    };
                    let mut emit_history = move |record: MatchHistory| {
                        if let Some(sender) = history_tx.as_ref() {
                            let _ = sender.send(record);
                        }
                    };
                    run_match_core(
                        &matchup,
                        &self.config,
                        &self.config.strategies,
                        &self.seed_deriver,
                        Some(&self.fast_models),
                        fast_eval_allowed,
                        total_matches,
                        log_events,
                        include_rounds,
                        &mut emit_event,
                        log_history,
                        &mut emit_history,
                        false,
                    )
                })
                .collect::<Vec<_>>()
        };

        let outcomes = run_with_parallelism(parallelism, run);

        if let Some(sender) = event_sender.as_ref() {
            let _ = sender.send(GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }

        let mut results = TournamentAccumulator::new(
            self.config.strategies.len(),
            self.config.engine.complexity_cost.enabled,
            self.config.engine.score_aggregation,
            !matches!(self.config.engine.mode, crate::config::EngineMode::Batch),
        );
        for outcome in outcomes {
            results.apply_match(
                outcome.result,
                outcome.a_crashed,
                outcome.b_crashed,
                outcome.a_tm_stats,
                outcome.b_tm_stats,
            );
        }
        runtime.note_cpu_matches(self.schedule.len());
        (results.finalize(&self.config.strategies), runtime)
    }
}
