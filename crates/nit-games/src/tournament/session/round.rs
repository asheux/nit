use super::super::metal::adjusted_total_for_match;
use super::super::types::{
    MatchOutcome, MatchResult, Matchup, RoundOutcome, RoundSnapshot, SeedDeriver,
};
use super::strategy_factory::strategy_log_id;
use super::MatchSession;
use crate::config::{NormalizedConfig, StrategySpec};
use crate::events::{EventWriter, GameEvent};
use crate::fast_eval::{evaluate_match, CycleMetadata, FastStrategyModel};
use crate::game::{payoffs_with_timeouts, Action, Outcome};
use crate::history_log::MatchHistory;
use crate::strategy::TmRunStats;
use nit_utils::rng::SplitMix64;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub(in crate::tournament) fn tm_metrics_from_stats(
    stats: &TmRunStats,
) -> crate::output::TmDerivedMetrics {
    let rounds = stats.rounds.max(1);
    let avg_steps = stats.steps as f64 / rounds as f64;
    let output_rate = stats.output_events as f64 / rounds as f64;
    let fallback_rate = stats.fallback as f64 / rounds as f64;
    crate::output::TmDerivedMetrics {
        rounds: stats.rounds,
        avg_steps_per_move: avg_steps,
        min_steps_per_move: stats.min_steps,
        max_steps_per_move: stats.max_steps,
        max_steps_hit_count: stats.max_steps_hits,
        output_event_hit_rate: output_rate,
        fallback_rate,
    }
}

// Strategies are user-provided; isolate their `next_action` calls so a panic
// in one player crashes only that side and the match continues with Defect.
fn safe_next_action(
    strategy: &mut dyn crate::strategy::Strategy,
    history: &crate::history::History,
    is_first_player: bool,
) -> (Action, bool, bool) {
    match catch_unwind(AssertUnwindSafe(|| {
        strategy.next_action(history, is_first_player)
    })) {
        Ok(action) => (action, strategy.last_halted(), false),
        Err(_) => (Action::Defect, false, true),
    }
}

pub(in crate::tournament) fn play_round_core(
    session: &mut MatchSession,
    config: &NormalizedConfig,
) -> RoundOutcome {
    let (mut a_action, a_halted, a_crash_now) = if session.a_crashed {
        (Action::Defect, false, false)
    } else {
        safe_next_action(session.a_strategy.as_mut(), &session.history, true)
    };
    let (mut b_action, b_halted, b_crash_now) = if session.b_crashed {
        (Action::Defect, false, false)
    } else {
        safe_next_action(session.b_strategy.as_mut(), &session.history, false)
    };

    if a_crash_now {
        session.a_crashed = true;
    }
    if b_crash_now {
        session.b_crashed = true;
    }

    a_action = apply_noise(config.noise, a_action, &mut session.noise_rng);
    b_action = apply_noise(config.noise, b_action, &mut session.noise_rng);
    let (a_payoff, b_payoff) =
        payoffs_with_timeouts(config.payoff, a_action, b_action, a_halted, b_halted);
    let outcome = Outcome::from_actions(a_action, b_action);
    session.a_total += a_payoff as i64;
    session.b_total += b_payoff as i64;
    session.history.push(a_action, b_action);
    if session.record_history || session.record_trace {
        session
            .history_scores
            .push(char::from(outcome.digit_byte()));
    }
    if session.record_trace {
        session.history_payoffs.push([a_payoff, b_payoff]);
    }
    if session.record_history {
        session.history_actions_a.push(a_action.as_char());
        session.history_actions_b.push(b_action.as_char());
        session
            .history_halted_a
            .push(if a_halted { '1' } else { '0' });
        session
            .history_halted_b
            .push(if b_halted { '1' } else { '0' });
    }
    session.round += 1;

    RoundOutcome {
        snapshot: RoundSnapshot {
            a_action,
            b_action,
            a_halted,
            b_halted,
            a_payoff,
            b_payoff,
        },
        a_crash_now,
        b_crash_now,
    }
}

fn apply_noise(noise: f32, action: Action, rng: &mut SplitMix64) -> Action {
    if noise <= 0.0 {
        return action;
    }
    if rng.next_f32() < noise {
        match action {
            Action::Cooperate => Action::Defect,
            Action::Defect => Action::Cooperate,
        }
    } else {
        action
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::tournament) fn run_match_core<E, H>(
    matchup: &Matchup,
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    seed_deriver: &SeedDeriver,
    fast_models: Option<&[Option<FastStrategyModel>]>,
    fast_eval_allowed: bool,
    total_matches: usize,
    log_events: bool,
    include_rounds: bool,
    emit_event: &mut E,
    log_history: bool,
    emit_history: &mut H,
    record_trace: bool,
) -> MatchOutcome
where
    E: FnMut(GameEvent),
    H: FnMut(MatchHistory),
{
    let a_id = strategy_log_id(&strategies[matchup.a_idx]);
    let b_id = strategy_log_id(&strategies[matchup.b_idx]);
    let a_spec = &strategies[matchup.a_idx];
    let b_spec = &strategies[matchup.b_idx];
    let cost = &config.engine.complexity_cost;
    let match_index = matchup.match_id + 1;
    let owned_ids = (log_events || log_history).then(|| (a_id.clone(), b_id.clone()));

    if log_events {
        let (a_owned, b_owned) = owned_ids.as_ref().expect("owned ids");
        emit_event(GameEvent::MatchStart {
            timestamp: EventWriter::timestamp(),
            match_id: matchup.match_id,
            match_index,
            total_matches,
            a: a_owned.clone(),
            b: b_owned.clone(),
            repetition: matchup.repetition + 1,
        });
    }

    if fast_eval_allowed && !record_trace {
        if let Some((a_model, b_model)) = fast_models.and_then(|models| {
            let a = models.get(matchup.a_idx).and_then(|m| m.as_ref());
            let b = models.get(matchup.b_idx).and_then(|m| m.as_ref());
            a.zip(b)
        }) {
            let record_cycle = log_history && config.history.include_cycle_metadata;
            let eval = evaluate_match(
                a_model,
                b_model,
                config.rounds,
                config.payoff,
                record_cycle,
                log_history,
            );
            if log_events {
                emit_event(GameEvent::MatchEnd {
                    timestamp: EventWriter::timestamp(),
                    match_id: matchup.match_id,
                    match_index,
                    a_total: eval.a_total,
                    b_total: eval.b_total,
                });
            }
            if log_history {
                let (a_owned, b_owned) = owned_ids.as_ref().expect("owned ids");
                emit_history(MatchHistory {
                    match_id: matchup.match_id,
                    match_index,
                    total_matches,
                    a: a_owned.clone(),
                    b: b_owned.clone(),
                    repetition: matchup.repetition + 1,
                    rounds: config.rounds,
                    score_idx: eval.outcomes.unwrap_or_default(),
                    a_score: eval.a_total,
                    b_score: eval.b_total,
                    cycle: eval.cycle.clone(),
                    a_tm_metrics: None,
                    b_tm_metrics: None,
                });
            }
            let a_adjusted_total =
                adjusted_total_for_match(eval.a_total, a_spec, config.rounds, None, cost);
            let b_adjusted_total =
                adjusted_total_for_match(eval.b_total, b_spec, config.rounds, None, cost);
            return MatchOutcome {
                result: MatchResult {
                    a_idx: matchup.a_idx,
                    b_idx: matchup.b_idx,
                    rounds: config.rounds,
                    a_total: eval.a_total,
                    b_total: eval.b_total,
                    a_adjusted_total,
                    b_adjusted_total,
                    repetition: matchup.repetition,
                    match_id: matchup.match_id,
                },
                a_crashed: false,
                b_crashed: false,
                a_tm_stats: None,
                b_tm_stats: None,
                last_round: None,
            };
        }
    }

    let mut session = MatchSession::new(
        matchup.clone(),
        config,
        strategies,
        seed_deriver,
        log_history,
        record_trace,
    );
    let mut last_round = None;
    for _ in 0..session.rounds_total {
        let outcome = play_round_core(&mut session, config);
        last_round = Some(outcome.snapshot.clone());
        if outcome.a_crash_now && log_events {
            emit_event(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: a_id.clone(),
                error: "panic in strategy".into(),
            });
        }
        if outcome.b_crash_now && log_events {
            emit_event(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: b_id.clone(),
                error: "panic in strategy".into(),
            });
        }
        if log_events && include_rounds {
            emit_event(GameEvent::Round {
                timestamp: EventWriter::timestamp(),
                match_id: matchup.match_id,
                match_index,
                round: session.round,
                a_action: outcome.snapshot.a_action.as_char(),
                b_action: outcome.snapshot.b_action.as_char(),
                a_halted: outcome.snapshot.a_halted,
                b_halted: outcome.snapshot.b_halted,
                a_payoff: outcome.snapshot.a_payoff,
                b_payoff: outcome.snapshot.b_payoff,
            });
        }
    }

    if log_events {
        emit_event(GameEvent::MatchEnd {
            timestamp: EventWriter::timestamp(),
            match_id: matchup.match_id,
            match_index,
            a_total: session.a_total,
            b_total: session.b_total,
        });
    }

    let mut cycle_meta: Option<CycleMetadata> = None;
    if log_history && config.history.include_cycle_metadata && config.noise == 0.0 {
        if let Some((a_model, b_model)) = fast_models.and_then(|models| {
            let a = models.get(matchup.a_idx).and_then(|m| m.as_ref());
            let b = models.get(matchup.b_idx).and_then(|m| m.as_ref());
            a.zip(b)
        }) {
            cycle_meta =
                evaluate_match(a_model, b_model, config.rounds, config.payoff, true, false).cycle;
        }
    }

    if log_history {
        let (a_owned, b_owned) = owned_ids.as_ref().expect("owned ids");
        let include_tm_metrics = config.history.include_cycle_metadata;
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
        emit_history(MatchHistory {
            match_id: matchup.match_id,
            match_index,
            total_matches,
            a: a_owned.clone(),
            b: b_owned.clone(),
            repetition: matchup.repetition + 1,
            rounds: session.rounds_total,
            score_idx: session.history_scores.clone(),
            a_score: session.a_total,
            b_score: session.b_total,
            cycle: cycle_meta,
            a_tm_metrics,
            b_tm_metrics,
        });
    }

    MatchOutcome {
        result: MatchResult {
            a_idx: matchup.a_idx,
            b_idx: matchup.b_idx,
            rounds: session.rounds_total,
            a_total: session.a_total,
            b_total: session.b_total,
            a_adjusted_total: adjusted_total_for_match(
                session.a_total,
                a_spec,
                session.rounds_total,
                session.a_strategy.tm_stats(),
                cost,
            ),
            b_adjusted_total: adjusted_total_for_match(
                session.b_total,
                b_spec,
                session.rounds_total,
                session.b_strategy.tm_stats(),
                cost,
            ),
            repetition: matchup.repetition,
            match_id: matchup.match_id,
        },
        a_crashed: session.a_crashed,
        b_crashed: session.b_crashed,
        a_tm_stats: session.a_strategy.tm_stats().cloned(),
        b_tm_stats: session.b_strategy.tm_stats().cloned(),
        last_round,
    }
}
