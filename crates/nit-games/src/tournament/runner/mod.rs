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
use crate::config::{EngineMode, NormalizedConfig, StrategySpec};
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
            !matches!(config.engine.mode, EngineMode::Batch),
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
        let mut remaining = steps;
        self.try_fast_forward_matches(&mut remaining);
        while remaining > 0 && !self.is_done() {
            if !self.advance_one_round() {
                break;
            }
            remaining = remaining.saturating_sub(1);
            self.try_fast_forward_matches(&mut remaining);
        }
    }

    // Drives a single round of progress. Returns false when no further work
    // is possible (schedule exhausted mid-step or no session can be opened).
    fn advance_one_round(&mut self) -> bool {
        if self.current.is_none() {
            let Some(next_matchup) = self.schedule.matchup(self.match_index) else {
                return false;
            };
            self.open_new_match(next_matchup);
        }
        let Some(mut session) = self.current.take() else {
            return false;
        };
        let snapshot = self.play_round(&mut session);
        self.last_round = Some(snapshot.clone());
        self.last_progress = Some(self.progress_for_active(&session, Some(&snapshot)));
        if session.round >= session.rounds_total {
            self.finalize_completed_session(session);
        } else {
            self.current = Some(session);
        }
        true
    }

    fn progress_for_active(
        &self,
        session: &MatchSession,
        last_snapshot: Option<&RoundSnapshot>,
    ) -> TournamentProgress {
        TournamentProgress::build(
            self.match_index.saturating_add(1),
            self.schedule.len().max(1),
            session.round,
            session.rounds_total,
            false,
            strategy_log_id(&self.strategies[session.matchup.a_idx]),
            strategy_log_id(&self.strategies[session.matchup.b_idx]),
            session.a_total,
            session.b_total,
            last_snapshot,
            self.runtime.clone(),
        )
    }

    fn open_new_match(&mut self, next_matchup: Matchup) {
        let session = MatchSession::new(
            next_matchup,
            &self.config,
            &self.strategies,
            &self.seed_deriver,
            true,
            true,
        );
        let player_a_id = self.strategies[session.matchup.a_idx].id.clone();
        let player_b_id = self.strategies[session.matchup.b_idx].id.clone();
        self.emit(GameEvent::MatchStart {
            timestamp: EventWriter::timestamp(),
            match_id: session.matchup.match_id,
            match_index: self.match_index + 1,
            total_matches: self.schedule.len(),
            a: player_a_id.clone(),
            b: player_b_id.clone(),
            repetition: session.matchup.repetition + 1,
        });
        self.last_progress = Some(TournamentProgress::build(
            self.match_index.saturating_add(1),
            self.schedule.len().max(1),
            0,
            session.rounds_total,
            false,
            player_a_id,
            player_b_id,
            0,
            0,
            None,
            self.runtime.clone(),
        ));
        self.current = Some(session);
    }

    fn finalize_completed_session(&mut self, session: MatchSession) {
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
        let match_result = MatchResult {
            a_idx: session.matchup.a_idx,
            b_idx: session.matchup.b_idx,
            rounds: session.rounds_total,
            a_total: session.a_total,
            b_total: session.b_total,
            a_adjusted_total: adjusted_total_for_match(
                session.a_total,
                a_spec,
                session.rounds_total,
                a_tm_stats,
                cost,
            ),
            b_adjusted_total: adjusted_total_for_match(
                session.b_total,
                b_spec,
                session.rounds_total,
                b_tm_stats,
                cost,
            ),
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
            result: match_result,
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
    }

    fn play_round(&mut self, session: &mut MatchSession) -> RoundSnapshot {
        self.runtime.note_cpu_activity();
        let a_id = self.strategies[session.matchup.a_idx].id.clone();
        let b_id = self.strategies[session.matchup.b_idx].id.clone();

        let outcome = play_round_core(session, &self.config);

        if outcome.a_crash_now {
            session.a_crashed = true;
            self.note_crash(session.matchup.a_idx, a_id);
        }
        if outcome.b_crash_now {
            session.b_crashed = true;
            self.note_crash(session.matchup.b_idx, b_id);
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

    fn note_crash(&mut self, strategy_idx: usize, strategy_id: String) {
        self.results.strategies[strategy_idx].crash_count += 1;
        self.results.strategies[strategy_idx].crashed = true;
        self.emit(GameEvent::StrategyError {
            timestamp: EventWriter::timestamp(),
            strategy_id,
            error: "panic in strategy".into(),
        });
    }
}
