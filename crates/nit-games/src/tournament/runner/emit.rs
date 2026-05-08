use super::TournamentRunner;
use crate::events::GameEvent;
use crate::history_log::MatchHistory;
use crate::output::{RunPaths, RunSummary};
use crate::tournament::session::{strategy_log_id, tm_metrics_from_stats};
use crate::tournament::types::MatchSession;

impl TournamentRunner {
    pub fn finish(mut self, timestamp: String, run_id: String, config_text: String) -> RunSummary {
        let event_log_path = take_log_path(self.event_writer.take().map(EventLog::Event));
        let history_log_path = take_log_path(self.history_writer.take().map(EventLog::History));
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
        let Some(writer) = self.event_writer.as_mut() else {
            return;
        };
        if matches!(event, GameEvent::Round { .. }) && !writer.include_rounds() {
            return;
        }
        let _ = writer.write(&event);
    }

    pub(super) fn emit_history(&mut self, session: &MatchSession) {
        let Some(writer) = self.history_writer.as_mut() else {
            return;
        };
        let include_tm_metrics = self.config.history.include_cycle_metadata;
        let history_record = MatchHistory {
            match_id: session.matchup.match_id,
            match_index: self.match_index + 1,
            total_matches: self.schedule.len(),
            a: strategy_log_id(&self.strategies[session.matchup.a_idx]),
            b: strategy_log_id(&self.strategies[session.matchup.b_idx]),
            repetition: session.matchup.repetition + 1,
            rounds: session.rounds_total,
            score_idx: session.history_scores.clone(),
            a_score: session.a_total,
            b_score: session.b_total,
            cycle: None,
            a_tm_metrics: tm_metrics_if(include_tm_metrics, session.a_strategy.tm_stats()),
            b_tm_metrics: tm_metrics_if(include_tm_metrics, session.b_strategy.tm_stats()),
        };
        let _ = writer.write(&history_record);
    }
}

fn tm_metrics_if(
    enabled: bool,
    stats: Option<&crate::strategy::TmRunStats>,
) -> Option<crate::output::TmDerivedMetrics> {
    if enabled {
        stats.map(tm_metrics_from_stats)
    } else {
        None
    }
}

enum EventLog {
    Event(crate::events::EventWriter),
    History(crate::history_log::HistoryWriter),
}

fn take_log_path(writer: Option<EventLog>) -> Option<String> {
    let path = match writer? {
        EventLog::Event(w) => w.finish().ok()?,
        EventLog::History(w) => w.finish().ok()?,
    };
    Some(path.to_string_lossy().to_string())
}
