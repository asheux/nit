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
        if let Some(session) = self.current.as_ref() {
            return self.progress_from_active_session(session);
        }
        if let Some(cached) = self.progress_from_batch_cache() {
            return Some(cached);
        }
        if let Some(matchup) = self.schedule.matchup(self.match_index) {
            let a_label = strategy_log_id(self.strategies.get(matchup.a_idx)?);
            let b_label = strategy_log_id(self.strategies.get(matchup.b_idx)?);
            return Some(TournamentProgress::build(
                self.match_index.saturating_add(1),
                self.schedule.len().max(1),
                0,
                self.config.rounds,
                false,
                a_label,
                b_label,
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
        session: &MatchSession,
    ) -> Option<TournamentProgress> {
        let matchup = &session.matchup;
        let a_label = strategy_log_id(self.strategies.get(matchup.a_idx)?);
        let b_label = strategy_log_id(self.strategies.get(matchup.b_idx)?);
        let last_snapshot = (session.round > 0)
            .then_some(self.last_round.as_ref())
            .flatten();
        Some(TournamentProgress::build(
            self.match_index.saturating_add(1),
            self.schedule.len().max(1),
            session.round,
            session.rounds_total,
            false,
            a_label,
            b_label,
            session.a_total,
            session.b_total,
            last_snapshot,
            self.runtime.clone(),
        ))
    }

    pub(super) fn progress_from_batch_cache(&self) -> Option<TournamentProgress> {
        if !matches!(self.config.engine.mode, crate::config::EngineMode::Batch) {
            return None;
        }
        let mut cached = self.last_progress.clone()?;
        cached.runtime = self.runtime.clone();
        Some(cached)
    }

    pub fn match_snapshot(&self) -> Option<MatchSnapshot> {
        let session = self.current.as_ref()?;
        let matchup = &session.matchup;
        Some(MatchSnapshot {
            match_index: self.match_index.saturating_add(1),
            total_matches: self.schedule.len().max(1),
            round: session.round,
            rounds: session.rounds_total,
            a: strategy_log_id(self.strategies.get(matchup.a_idx)?),
            b: strategy_log_id(self.strategies.get(matchup.b_idx)?),
            a_score: session.a_total,
            b_score: session.b_total,
            outcomes: session.history_scores.clone(),
            payoffs: session.history_payoffs.clone(),
            a_halted: session.history_halted_a.clone(),
            b_halted: session.history_halted_b.clone(),
        })
    }
}
