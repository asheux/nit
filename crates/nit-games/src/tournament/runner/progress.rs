use super::{TournamentRunner, EMPTY_SCHEDULE_SENTINEL};
use crate::tournament::session::strategy_log_id;
use crate::tournament::types::{MatchSession, MatchSnapshot, TournamentProgress};

impl TournamentRunner {
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

    pub(super) fn progress_from_active_session(
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

    pub(super) fn progress_from_batch_cache(&self) -> Option<TournamentProgress> {
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
}
