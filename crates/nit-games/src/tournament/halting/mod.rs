mod metal_path;
mod notebook_eval;

use super::schedule::{total_schedule_matches, SchedulePlan};
use super::session::run_match_core;
use super::types::{
    run_with_parallelism, MatchOutcome, Matchup, Parallelism, SeedDeriver, TmHaltingFilterBackend,
    TmHaltingFilterDiagnostics,
};
use crate::config::{NormalizedConfig, StrategySpec, StrategySpecKind};
use crate::events::GameEvent;
use crate::history_log::MatchHistory;
use crate::strategy::TmRunStats;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::time::Instant;

const SCORE_EPSILON: f64 = 1e-9;

pub(super) fn compare_scores(a: f64, b: f64) -> Ordering {
    let diff = (a - b).abs();
    if diff < SCORE_EPSILON {
        Ordering::Equal
    } else if a > b {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

fn strategy_is_one_sided_tm(spec: &StrategySpec) -> bool {
    matches!(spec.kind, StrategySpecKind::OneSidedTm { .. })
}

fn roster_is_all_tms(strategies: &[StrategySpec]) -> bool {
    !strategies.is_empty() && strategies.iter().all(strategy_is_one_sided_tm)
}

pub(super) fn tm_stats_always_halt(stats: Option<&TmRunStats>) -> bool {
    stats.is_some_and(|s| s.fallback == 0 && s.output_events == s.rounds)
}

enum RosterKind {
    AllTm,
    Mixed { tm_mask: Vec<bool> },
    NoTm,
}

fn classify_roster(strategies: &[StrategySpec]) -> RosterKind {
    if roster_is_all_tms(strategies) {
        return RosterKind::AllTm;
    }
    let tm_mask: Vec<bool> = strategies.iter().map(strategy_is_one_sided_tm).collect();
    if tm_mask.iter().any(|&is_tm| is_tm) {
        RosterKind::Mixed { tm_mask }
    } else {
        RosterKind::NoTm
    }
}

fn all_tm_halting_mask(
    config: &NormalizedConfig,
    strict_metal: bool,
    diagnostics: &mut TmHaltingFilterDiagnostics,
) -> Result<Vec<bool>, String> {
    let strategy_count = config.strategies.len();
    let attempted_metal = config.engine.accelerator.allows_metal();

    if attempted_metal {
        let probe_started = Instant::now();
        let schedule_len =
            SchedulePlan::new(strategy_count, config.repetitions, config.self_play).len();

        if let Some(decline_reason) =
            super::metal::metal_batch_decline_reason(config, &config.strategies, schedule_len)
        {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_decline_reason = Some(decline_reason.clone());
            if strict_metal && config.engine.accelerator.requires_metal() {
                return Err(format!(
                    "Metal accelerator was requested, but {decline_reason}."
                ));
            }
        } else if let Some(probe_result) =
            metal_path::dispatch_probe(config, diagnostics, probe_started)
        {
            let applied = metal_path::apply_probe_result(
                probe_result,
                config,
                schedule_len,
                strict_metal,
                diagnostics,
                probe_started,
            )?;
            if let Some(halting_keep) = applied {
                return Ok(halting_keep);
            }
        }
    }

    let filter_started = Instant::now();
    let (keep, notebook_stats) = notebook_eval::family_halting_mask(config);
    diagnostics.backend = if attempted_metal {
        TmHaltingFilterBackend::NotebookCpuFallback
    } else {
        TmHaltingFilterBackend::NotebookCpu
    };
    diagnostics.halting_filter_elapsed = filter_started.elapsed();
    diagnostics.scanned_matchups = notebook_stats.scanned_matchups;
    diagnostics.tm_cache_hits = notebook_stats.tm_cache_hits;
    diagnostics.tm_cache_misses = notebook_stats.tm_cache_misses;
    diagnostics.tm_evaluations = notebook_stats.tm_evaluations;
    diagnostics.tm_steps = notebook_stats.tm_steps;
    Ok(keep)
}

fn halting_turing_machine_mask(
    config: &NormalizedConfig,
    seed: u64,
    strict_metal: bool,
    diagnostics: &mut TmHaltingFilterDiagnostics,
) -> Result<Vec<bool>, String> {
    let strategy_count = config.strategies.len();
    diagnostics.schedule_matches =
        total_schedule_matches(strategy_count, config.repetitions, config.self_play).unwrap_or(0);

    let tm_mask = match classify_roster(&config.strategies) {
        RosterKind::AllTm => {
            return all_tm_halting_mask(config, strict_metal, diagnostics);
        }
        RosterKind::NoTm => {
            diagnostics.backend = TmHaltingFilterBackend::NotRequired;
            return Ok(vec![true; strategy_count]);
        }
        RosterKind::Mixed { tm_mask } => tm_mask,
    };

    diagnostics.backend = TmHaltingFilterBackend::MixedRosterCpu;
    let schedule = SchedulePlan::new(strategy_count, config.repetitions, config.self_play);
    if schedule.is_empty() {
        return Ok(vec![true; strategy_count]);
    }
    let scanned_matchups = (0..schedule.len())
        .filter(|match_id| {
            let matchup = schedule
                .matchup(*match_id)
                .expect("matchup should exist for in-range id");
            tm_mask[matchup.a_idx] || tm_mask[matchup.b_idx]
        })
        .count();
    diagnostics.scanned_matchups = scanned_matchups;

    let seed_deriver = SeedDeriver::new(seed);
    let total_matches = schedule.len();
    let filter_started = Instant::now();
    let evaluate_matchup = |matchup: &Matchup| -> MatchOutcome {
        let mut emit_event = |_event: GameEvent| {};
        let mut emit_history = |_record: MatchHistory| {};
        run_match_core(
            matchup,
            config,
            &config.strategies,
            &seed_deriver,
            None,
            false,
            total_matches,
            false,
            false,
            &mut emit_event,
            false,
            &mut emit_history,
            false,
        )
    };

    let mut keep = vec![true; strategy_count];
    let scan_parallel = || {
        (0..total_matches)
            .into_par_iter()
            .fold(
                || vec![true; strategy_count],
                |mut local_keep, match_id| {
                    let matchup = schedule
                        .matchup(match_id)
                        .expect("matchup should exist for in-range id");
                    if tm_mask[matchup.a_idx] || tm_mask[matchup.b_idx] {
                        let outcome = evaluate_matchup(&matchup);
                        metal_path::mark_tm_halting_selection(
                            &mut local_keep,
                            &tm_mask,
                            &matchup,
                            &outcome,
                        );
                    }
                    local_keep
                },
            )
            .reduce(
                || vec![true; strategy_count],
                |mut left, right| {
                    for (slot, keep_right) in left.iter_mut().zip(right) {
                        *slot &= keep_right;
                    }
                    left
                },
            )
    };

    keep = match Parallelism::from_config(&config.engine.parallelism) {
        Parallelism::Off => {
            for match_id in 0..total_matches {
                let matchup = schedule
                    .matchup(match_id)
                    .expect("matchup should exist for in-range id");
                if !tm_mask[matchup.a_idx] && !tm_mask[matchup.b_idx] {
                    continue;
                }
                let outcome = evaluate_matchup(&matchup);
                metal_path::mark_tm_halting_selection(&mut keep, &tm_mask, &matchup, &outcome);
            }
            keep
        }
        other => run_with_parallelism(other, scan_parallel),
    };

    diagnostics.halting_filter_elapsed = filter_started.elapsed();
    Ok(keep)
}

fn select_halting_turing_machine_strategies_inner(
    mut config: NormalizedConfig,
    strict_metal: bool,
) -> Result<(NormalizedConfig, TmHaltingFilterDiagnostics), String> {
    let selection_started = Instant::now();
    let mut diagnostics = TmHaltingFilterDiagnostics {
        strategy_count_before: config.strategies.len(),
        strategy_count_after: config.strategies.len(),
        backend: TmHaltingFilterBackend::NotRequired,
        requested_accelerator: config.engine.accelerator,
        ..TmHaltingFilterDiagnostics::default()
    };
    if config.tm_filter_applied {
        diagnostics.backend = TmHaltingFilterBackend::NotApplied;
        diagnostics.total_elapsed = selection_started.elapsed();
        return Ok((config, diagnostics));
    }

    let seed = config.seed.unwrap_or(0);
    config.seed = Some(seed);

    let keep = halting_turing_machine_mask(&config, seed, strict_metal, &mut diagnostics)?;
    if keep.iter().any(|&entry| !entry) {
        config.strategies = config
            .strategies
            .into_iter()
            .zip(keep.iter())
            .filter_map(|(spec, &kept)| kept.then_some(spec))
            .collect();
    }
    config.tm_filter_applied = true;
    diagnostics.strategy_count_after = config.strategies.len();
    diagnostics.total_elapsed = selection_started.elapsed();
    Ok((config, diagnostics))
}

pub fn try_select_halting_turing_machine_strategies_with_diagnostics(
    config: NormalizedConfig,
) -> Result<(NormalizedConfig, TmHaltingFilterDiagnostics), String> {
    select_halting_turing_machine_strategies_inner(config, true)
}

pub fn try_select_halting_turing_machine_strategies(
    config: NormalizedConfig,
) -> Result<NormalizedConfig, String> {
    try_select_halting_turing_machine_strategies_with_diagnostics(config).map(|(config, _)| config)
}

pub fn select_halting_turing_machine_strategies(config: NormalizedConfig) -> NormalizedConfig {
    select_halting_turing_machine_strategies_inner(config, false)
        .map(|(config, _)| config)
        .expect("non-strict TM halting selection always falls back to CPU")
}
