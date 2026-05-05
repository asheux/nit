use super::TournamentRunner;
use crate::events::GameEvent;
use crate::history_log::MatchHistory;
use crate::output::{RunPaths, RunSummary};
use crate::tournament::session::{strategy_log_id, tm_metrics_from_stats};
use crate::tournament::types::MatchSession;

impl TournamentRunner {
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
            paths: RunPaths {
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

    pub(super) fn emit(&mut self, event: GameEvent) {
        let Some(event_log_writer) = self.event_writer.as_mut() else {
            return;
        };
        if matches!(event, GameEvent::Round { .. }) && !event_log_writer.include_rounds() {
            return;
        }
        let _ = event_log_writer.write(&event);
    }

    pub(super) fn emit_history(&mut self, finished_session: &MatchSession) {
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
}
