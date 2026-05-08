//! `OneSidedTmStrategy` — drives a one-sided TM each round on a sliding
//! base-`symbols` window of the joint action history.

use crate::game::Action;
use crate::history::History;
use crate::strategy::{symbol_to_action, Strategy, TmTransition};

use super::engine::run_one_sided_tm;
use super::input_suffix::InputSuffix;
use super::{TmRunStats, TmStopReason};

#[derive(Clone, Debug)]
pub struct OneSidedTmStrategy {
    id: String,
    symbols: u8,
    start_state: u16,
    blank: u8,
    max_steps_per_round: u32,
    transitions: Vec<TmTransition>,
    input_suffix: InputSuffix,
    last_halted: bool,
    stats: TmRunStats,
}

impl OneSidedTmStrategy {
    pub fn new(
        id: impl Into<String>,
        symbols: u8,
        start_state: u16,
        blank: u8,
        max_steps_per_round: u32,
        transitions: Vec<TmTransition>,
    ) -> Self {
        let symbols = symbols.max(2);
        let width = max_steps_per_round as usize + 1;
        Self {
            id: id.into(),
            symbols,
            start_state,
            blank,
            max_steps_per_round,
            transitions,
            input_suffix: InputSuffix::new(symbols, width.max(1)),
            last_halted: true,
            stats: TmRunStats::default(),
        }
    }

    pub fn stats(&self) -> &TmRunStats {
        &self.stats
    }

    /// Bring the input suffix up to date with the current history.
    ///
    /// When exactly one round was appended since the last sync we push
    /// incrementally; otherwise we rebuild from scratch (covers history
    /// resets and skipped rounds).
    fn sync_input(&mut self, history: &History) {
        let len = history.len();
        let prev = self.input_suffix.history_len;

        if len == prev + 1 {
            if let Some(last) = history.last() {
                self.input_suffix.push_round(last);
            }
        } else if len != prev {
            self.input_suffix.reset();
            for round in history.iter() {
                self.input_suffix.push_round(round);
            }
        }

        self.input_suffix.history_len = len;
    }

    fn record_round(
        &mut self,
        steps_taken: u32,
        halted: bool,
        output_event: bool,
        max_steps_hit: bool,
    ) {
        self.last_halted = halted;
        self.stats.update_step_range(steps_taken, steps_taken);
        self.stats.rounds = self.stats.rounds.saturating_add(1);
        self.stats.steps = self.stats.steps.saturating_add(steps_taken as u64);
        if output_event {
            self.stats.output_events = self.stats.output_events.saturating_add(1);
        } else {
            self.stats.fallback = self.stats.fallback.saturating_add(1);
            if max_steps_hit {
                self.stats.max_steps_hits = self.stats.max_steps_hits.saturating_add(1);
            }
        }
    }
}

impl Strategy for OneSidedTmStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.input_suffix.reset();
        self.last_halted = true;
        self.stats = TmRunStats::default();
    }

    fn next_action(&mut self, history: &History, _player_a: bool) -> Action {
        if history.is_empty() {
            self.record_round(0, true, true, false);
            return Action::Cooperate;
        }

        self.sync_input(history);
        let input_digits = self.input_suffix.msd_digits();
        let run = run_one_sided_tm(
            &self.transitions,
            self.symbols,
            self.start_state,
            self.blank,
            &input_digits,
            self.max_steps_per_round,
            false,
        );

        self.record_round(
            run.steps_taken,
            run.halted,
            run.halted,
            matches!(run.stop_reason, TmStopReason::MaxSteps),
        );

        if let Some(symbol) = run.output_symbol {
            symbol_to_action(symbol)
        } else {
            Action::Defect
        }
    }

    fn last_halted(&self) -> bool {
        self.last_halted
    }

    fn tm_stats(&self) -> Option<&TmRunStats> {
        Some(&self.stats)
    }
}
