mod emit;
mod fast_forward;
mod progress;

use super::halting::select_halting_turing_machine_strategies;
use super::metal::adjusted_total_for_match;
use super::schedule::SchedulePlan;
use super::session::{build_strategy_definitions, play_round_core, strategy_log_id};
use super::types::{
    MatchHistoryPreview, MatchOutcome, MatchResult, MatchSession, Matchup, MetalBatchState,
    RoundSnapshot, SeedDeriver, TournamentAccumulator, TournamentProgress,
};
use crate::config::{NormalizedConfig, StrategySpec};
use crate::events::{EventWriter, GameEvent};
use crate::fast_eval::FastStrategyModel;
use crate::history_log::HistoryWriter;
use crate::output::{RuntimeAcceleratorStats, StrategyDefinition, TournamentResults};
use std::time::Instant;

const SNAPSHOT_REFRESH_MS: u64 = 100;
const EMPTY_SCHEDULE_SENTINEL: usize = 0;

pub struct TournamentRunner {
    pub(super) config: NormalizedConfig,
    pub(super) seed: u64,
    pub(super) schedule: SchedulePlan,
    pub(super) match_index: usize,
    pub(super) current: Option<MatchSession>,
    pub(super) results: TournamentAccumulator,
    pub(super) strategies: Vec<StrategySpec>,
    pub(super) definitions: Vec<StrategyDefinition>,
    pub(super) seed_deriver: SeedDeriver,
    pub(super) fast_models: Vec<Option<FastStrategyModel>>,
    pub(super) event_writer: Option<EventWriter>,
    pub(super) history_writer: Option<HistoryWriter>,
    pub(super) last_round: Option<RoundSnapshot>,
    pub(super) last_progress: Option<TournamentProgress>,
    pub(super) runtime: RuntimeAcceleratorStats,
    pub(super) metal_batch: MetalBatchState,
    pub(super) collect_match_history_previews: bool,
    pub(super) completed_history_previews: Vec<MatchHistoryPreview>,
    pub(super) last_snapshot_sample_at: Option<Instant>,
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
