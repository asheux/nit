mod round;
mod strategy_factory;

pub(super) use round::{play_round_core, run_match_core, tm_metrics_from_stats};
pub(super) use strategy_factory::{build_strategy_definitions, strategy_log_id};

#[cfg(all(test, target_os = "macos"))]
pub(crate) use strategy_factory::build_strategy;

use super::types::{MatchRole, MatchSession, Matchup, SeedDeriver};
use crate::config::{NormalizedConfig, StrategySpec};
use crate::history::History;
use nit_utils::rng::SplitMix64;

impl MatchSession {
    pub(in crate::tournament) fn new(
        matchup: Matchup,
        config: &NormalizedConfig,
        strategies: &[StrategySpec],
        seed_deriver: &SeedDeriver,
        record_history: bool,
        record_trace: bool,
    ) -> Self {
        let rounds_total = config.rounds;
        let max_memory = config.max_memory_n;
        let a_spec = &strategies[matchup.a_idx];
        let b_spec = &strategies[matchup.b_idx];
        let a_seed = seed_deriver.strategy_seed(
            matchup.match_id,
            matchup.repetition,
            MatchRole::A,
            &a_spec.id,
        );
        let b_seed = seed_deriver.strategy_seed(
            matchup.match_id,
            matchup.repetition,
            MatchRole::B,
            &b_spec.id,
        );
        let mut a_strategy = strategy_factory::build_strategy(a_spec, a_seed);
        let mut b_strategy = strategy_factory::build_strategy(b_spec, b_seed);
        a_strategy.reset();
        b_strategy.reset();
        let noise_seed = seed_deriver.noise_seed(matchup.match_id, matchup.repetition);
        let record_scores = record_history || record_trace;
        let history_buffer = |required: bool| {
            if required {
                String::with_capacity(rounds_total as usize)
            } else {
                String::new()
            }
        };
        Self {
            matchup,
            history: History::new(max_memory),
            a_strategy,
            b_strategy,
            noise_rng: SplitMix64::new(noise_seed),
            history_actions_a: history_buffer(record_history),
            history_actions_b: history_buffer(record_history),
            history_halted_a: history_buffer(record_history),
            history_halted_b: history_buffer(record_history),
            history_scores: history_buffer(record_scores),
            history_payoffs: if record_trace {
                Vec::with_capacity(rounds_total as usize)
            } else {
                Vec::new()
            },
            round: 0,
            rounds_total,
            a_total: 0,
            b_total: 0,
            a_crashed: false,
            b_crashed: false,
            record_history,
            record_trace,
        }
    }
}
