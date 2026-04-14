use super::metal::{metal_batch_decline_reason, try_prepare_metal_batch_for_workload};
use super::schedule::{matches_per_repetition, total_schedule_matches, SchedulePlan};
use super::session::run_match_core;
use super::types::{
    run_with_parallelism, MatchOutcome, Matchup, Parallelism, SeedDeriver, TmHaltingFilterBackend,
    TmHaltingFilterDiagnostics,
};
use crate::config::{AcceleratorMode, NormalizedConfig, StrategySpec, StrategySpecKind};
use crate::events::GameEvent;
use crate::game::Action;
use crate::history_log::MatchHistory;
use crate::strategy::{run_one_sided_tm, symbol_to_action, InputSuffix, TmRunStats, TmTransition};
use nit_metal::MatchPair;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const AUTO_TM_METAL_PROBE_TIMEOUT: Duration = Duration::from_millis(300);
const SCORE_EPSILON: f64 = 1e-9;
static AUTO_TM_METAL_PROBE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

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

fn tm_stats_always_halt(stats: Option<&TmRunStats>) -> bool {
    stats.is_some_and(|s| s.fallback == 0 && s.output_events == s.rounds)
}

#[derive(Copy, Clone)]
struct NotebookTmSpec<'a> {
    symbols: u8,
    start_state: u16,
    blank: u8,
    max_steps_per_round: u32,
    transitions: &'a [TmTransition],
}

fn notebook_tm_spec(spec: &StrategySpec) -> Option<NotebookTmSpec<'_>> {
    match &spec.kind {
        StrategySpecKind::OneSidedTm {
            symbols,
            start_state,
            blank,
            max_steps_per_round,
            transitions,
            ..
        } => Some(NotebookTmSpec {
            symbols: *symbols,
            start_state: *start_state,
            blank: *blank,
            max_steps_per_round: *max_steps_per_round,
            transitions,
        }),
        _ => None,
    }
}

#[derive(Copy, Clone)]
struct NotebookTmActionResult {
    action: Action,
    halted: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct NotebookTmFamilyStats {
    scanned_matchups: usize,
    tm_cache_hits: u64,
    tm_cache_misses: u64,
    tm_evaluations: u64,
    tm_steps: u64,
}

#[derive(Default)]
struct NotebookTmEvalCounters {
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    tm_evaluations: AtomicU64,
    tm_steps: AtomicU64,
}

struct NotebookTmEvaluationCache {
    per_strategy: Vec<Mutex<HashMap<Vec<u8>, NotebookTmActionResult>>>,
    counters: NotebookTmEvalCounters,
}

impl NotebookTmEvaluationCache {
    fn new(strategy_count: usize) -> Self {
        Self {
            per_strategy: (0..strategy_count)
                .map(|_| Mutex::new(HashMap::new()))
                .collect(),
            counters: NotebookTmEvalCounters::default(),
        }
    }

    fn lookup(&self, strategy_idx: usize, input_digits: &[u8]) -> Option<NotebookTmActionResult> {
        self.per_strategy
            .get(strategy_idx)?
            .lock()
            .expect("TM cache lock poisoned")
            .get(input_digits)
            .copied()
    }

    fn store(&self, strategy_idx: usize, input_digits: Vec<u8>, result: NotebookTmActionResult) {
        let Some(bucket) = self.per_strategy.get(strategy_idx) else {
            return;
        };
        bucket
            .lock()
            .expect("TM cache lock poisoned")
            .entry(input_digits)
            .or_insert(result);
    }

    fn snapshot(&self) -> NotebookTmFamilyStats {
        NotebookTmFamilyStats {
            scanned_matchups: 0,
            tm_cache_hits: self.counters.cache_hits.load(AtomicOrdering::Relaxed),
            tm_cache_misses: self.counters.cache_misses.load(AtomicOrdering::Relaxed),
            tm_evaluations: self.counters.tm_evaluations.load(AtomicOrdering::Relaxed),
            tm_steps: self.counters.tm_steps.load(AtomicOrdering::Relaxed),
        }
    }
}

fn notebook_tm_action_cached(
    strategy_idx: usize,
    spec: NotebookTmSpec<'_>,
    input: &InputSuffix,
    first_round: bool,
    cache: &NotebookTmEvaluationCache,
) -> (Action, bool) {
    if first_round {
        return (Action::Cooperate, true);
    }

    let input_digits = input.msd_digits();
    if let Some(cached) = cache.lookup(strategy_idx, &input_digits) {
        cache
            .counters
            .cache_hits
            .fetch_add(1, AtomicOrdering::Relaxed);
        return (cached.action, cached.halted);
    }

    cache
        .counters
        .cache_misses
        .fetch_add(1, AtomicOrdering::Relaxed);
    let run = run_one_sided_tm(
        spec.transitions,
        spec.symbols,
        spec.start_state,
        spec.blank,
        &input_digits,
        spec.max_steps_per_round,
        false,
    );
    cache
        .counters
        .tm_evaluations
        .fetch_add(1, AtomicOrdering::Relaxed);
    cache
        .counters
        .tm_steps
        .fetch_add(run.steps_taken as u64, AtomicOrdering::Relaxed);

    let action = run
        .output_symbol
        .map(symbol_to_action)
        .unwrap_or(Action::Defect);
    let result = NotebookTmActionResult {
        action,
        halted: run.halted,
    };
    cache.store(strategy_idx, input_digits, result);
    (result.action, result.halted)
}

fn notebook_tm_history_bit(action: Action, halted: bool) -> u8 {
    u8::from(halted && matches!(action, Action::Defect))
}

fn notebook_tm_matchup_halts_all_rounds(
    a_idx: usize,
    b_idx: usize,
    tm_specs: &[NotebookTmSpec<'_>],
    rounds: u32,
    cache: &NotebookTmEvaluationCache,
) -> (bool, bool) {
    let a_tm = tm_specs[a_idx];
    let b_tm = tm_specs[b_idx];
    let mut a_input = InputSuffix::new(a_tm.symbols, a_tm.max_steps_per_round as usize + 1);
    let mut b_input = InputSuffix::new(b_tm.symbols, b_tm.max_steps_per_round as usize + 1);
    let mut a_keep = true;
    let mut b_keep = true;

    for round in 0..rounds {
        let first_round = round == 0;
        let (a_action, a_halted) =
            notebook_tm_action_cached(a_idx, a_tm, &a_input, first_round, cache);
        let (b_action, b_halted) =
            notebook_tm_action_cached(b_idx, b_tm, &b_input, first_round, cache);
        a_keep &= a_halted;
        b_keep &= b_halted;

        let a_bit = notebook_tm_history_bit(a_action, a_halted);
        let b_bit = notebook_tm_history_bit(b_action, b_halted);
        a_input.push_pair_bits(a_bit, b_bit);
        b_input.push_pair_bits(a_bit, b_bit);

        if !a_keep && !b_keep {
            break;
        }
    }

    (a_keep, b_keep)
}

fn notebook_tm_family_halting_mask(
    config: &NormalizedConfig,
) -> (Vec<bool>, NotebookTmFamilyStats) {
    let strategy_count = config.strategies.len();
    let mut keep = vec![true; strategy_count];
    let schedule = SchedulePlan::new(strategy_count, config.repetitions, config.self_play);
    if schedule.is_empty() {
        return (keep, NotebookTmFamilyStats::default());
    }
    let tm_specs = config
        .strategies
        .iter()
        .map(|spec| notebook_tm_spec(spec).expect("TM roster should only contain TM strategies"))
        .collect::<Vec<_>>();
    // TM-vs-TM halting outcomes are deterministic, so repetitions repeat the same ordered
    // matchup work. Scan one repetition and reuse the result across all repetitions.
    let scanned_matchups = matches_per_repetition(strategy_count, config.self_play).unwrap_or(0);
    if scanned_matchups == 0 {
        return (keep, NotebookTmFamilyStats::default());
    }
    let cache = Arc::new(NotebookTmEvaluationCache::new(strategy_count));

    let evaluate_matchup = |match_id: usize, keep: &mut [bool]| {
        let matchup = schedule
            .matchup(match_id)
            .expect("matchup should exist for in-range id");
        let (a_keep, b_keep) = notebook_tm_matchup_halts_all_rounds(
            matchup.a_idx,
            matchup.b_idx,
            &tm_specs,
            config.rounds,
            &cache,
        );
        if !a_keep {
            keep[matchup.a_idx] = false;
        }
        if !b_keep {
            keep[matchup.b_idx] = false;
        }
    };

    let scan_parallel = || {
        (0..scanned_matchups)
            .into_par_iter()
            .fold(
                || vec![true; strategy_count],
                |mut local_keep, match_id| {
                    evaluate_matchup(match_id, &mut local_keep);
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
            for match_id in 0..scanned_matchups {
                evaluate_matchup(match_id, &mut keep);
            }
            keep
        }
        other => run_with_parallelism(other, scan_parallel),
    };

    let mut stats = cache.snapshot();
    stats.scanned_matchups = scanned_matchups;
    (keep, stats)
}

fn apply_tm_family_halting_chunk(
    keep: &mut [bool],
    matchups: &[Matchup],
    halting: &[nit_metal::TmHaltingPair],
) -> Result<(), String> {
    if matchups.len() != halting.len() {
        return Err(format!(
            "Metal TM halting batch returned {} results for {} matchups",
            halting.len(),
            matchups.len()
        ));
    }
    for (matchup, outcome) in matchups.iter().zip(halting.iter()) {
        if !outcome.a_all_halted {
            keep[matchup.a_idx] = false;
        }
        if !outcome.b_all_halted {
            keep[matchup.b_idx] = false;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct MetalTmHaltingStats {
    scanned_matchups: usize,
    batches_submitted: usize,
    prepare_elapsed: Duration,
    execution_elapsed: Duration,
    policy_source: String,
    matches_per_batch: usize,
    inflight_batches: usize,
    policy_cache_key: Option<String>,
    policy_cache_path: Option<String>,
}

fn try_metal_tm_family_halting_mask(
    config: &NormalizedConfig,
) -> Result<Option<(Vec<bool>, MetalTmHaltingStats)>, String> {
    struct PendingChunk {
        matchups: Vec<Matchup>,
        pending: nit_metal::PendingBatch,
    }

    let strategy_count = config.strategies.len();
    let mut keep = vec![true; strategy_count];
    let schedule = SchedulePlan::new(strategy_count, config.repetitions, config.self_play);
    if schedule.is_empty() {
        return Ok(Some((keep, MetalTmHaltingStats::default())));
    }

    let prepare_started = Instant::now();
    let Some(prepared) =
        try_prepare_metal_batch_for_workload(config, &config.strategies, schedule.len())?
    else {
        return Ok(None);
    };
    let prepare_elapsed = prepare_started.elapsed();

    let policy_source = format!("{:?}", prepared.policy_source);
    let policy_cache_key = prepared.policy_cache_key.clone();
    let policy_cache_path = prepared.policy_cache_path.clone();
    let matches_per_batch = prepared.policy.matches_per_batch.max(1);
    let inflight_batches = prepared.policy.inflight_batches.max(1);
    let mut pending = VecDeque::new();
    let mut batches_submitted = 0usize;
    let execution_started = Instant::now();

    for start in (0..schedule.len()).step_by(matches_per_batch) {
        let matchups = schedule.matchups(start, matches_per_batch);
        if matchups.is_empty() {
            continue;
        }
        let pairs = matchups
            .iter()
            .map(|matchup| MatchPair {
                a_idx: matchup.a_idx as u32,
                b_idx: matchup.b_idx as u32,
            })
            .collect::<Vec<_>>();
        let Some(batch) = nit_metal::try_begin_prepared_batch(&prepared.prepared, &pairs)? else {
            return Ok(None);
        };
        pending.push_back(PendingChunk {
            matchups,
            pending: batch,
        });
        batches_submitted += 1;
        if pending.len() >= inflight_batches {
            let ready = pending.pop_front().expect("pending TM halting chunk");
            let halting = nit_metal::try_finish_prepared_tm_halting_batch(ready.pending)?;
            apply_tm_family_halting_chunk(&mut keep, &ready.matchups, &halting)?;
        }
    }

    while let Some(ready) = pending.pop_front() {
        let halting = nit_metal::try_finish_prepared_tm_halting_batch(ready.pending)?;
        apply_tm_family_halting_chunk(&mut keep, &ready.matchups, &halting)?;
    }

    Ok(Some((
        keep,
        MetalTmHaltingStats {
            scanned_matchups: schedule.len(),
            batches_submitted,
            prepare_elapsed,
            execution_elapsed: execution_started.elapsed(),
            policy_source,
            matches_per_batch,
            inflight_batches,
            policy_cache_key,
            policy_cache_path,
        },
    )))
}

fn mark_tm_halting_selection(
    keep: &mut [bool],
    tm_mask: &[bool],
    matchup: &Matchup,
    outcome: &MatchOutcome,
) {
    if tm_mask[matchup.a_idx] && !tm_stats_always_halt(outcome.a_tm_stats.as_ref()) {
        keep[matchup.a_idx] = false;
    }
    if tm_mask[matchup.b_idx] && !tm_stats_always_halt(outcome.b_tm_stats.as_ref()) {
        keep[matchup.b_idx] = false;
    }
}

type MetalProbeResult = Result<Option<(Vec<bool>, MetalTmHaltingStats)>, String>;

fn dispatch_metal_tm_probe(
    config: &NormalizedConfig,
    diagnostics: &mut TmHaltingFilterDiagnostics,
    probe_started: Instant,
) -> Option<MetalProbeResult> {
    if !matches!(config.engine.accelerator, AcceleratorMode::Auto) {
        return Some(try_metal_tm_family_halting_mask(config));
    }

    if AUTO_TM_METAL_PROBE_IN_FLIGHT.swap(true, AtomicOrdering::AcqRel) {
        diagnostics.backend_probe_elapsed = probe_started.elapsed();
        diagnostics.metal_decline_reason =
            Some("Metal probe already in progress; using CPU fallback for this run.".into());
        return None;
    }

    let (sender, receiver) = std::sync::mpsc::channel();
    let probe_cfg = config.clone();
    std::thread::spawn(move || {
        let probe_outcome = catch_unwind(AssertUnwindSafe(|| {
            try_metal_tm_family_halting_mask(&probe_cfg)
        }))
        .unwrap_or_else(|_| Err("Metal probe panicked".into()));
        AUTO_TM_METAL_PROBE_IN_FLIGHT.store(false, AtomicOrdering::Release);
        let _ = sender.send(probe_outcome);
    });

    match receiver.recv_timeout(AUTO_TM_METAL_PROBE_TIMEOUT) {
        Ok(outcome) => Some(outcome),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_decline_reason = Some(format!(
                "Metal probe exceeded {}ms in auto mode; using CPU fallback",
                AUTO_TM_METAL_PROBE_TIMEOUT.as_millis()
            ));
            None
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_error = Some("Metal probe thread terminated unexpectedly".into());
            None
        }
    }
}

fn apply_metal_probe_result(
    probe_result: MetalProbeResult,
    config: &NormalizedConfig,
    schedule_len: usize,
    strict_metal: bool,
    diagnostics: &mut TmHaltingFilterDiagnostics,
    probe_started: Instant,
) -> Result<Option<Vec<bool>>, String> {
    match probe_result {
        Ok(Some((halting_keep, gpu_stats))) => {
            diagnostics.backend = TmHaltingFilterBackend::Metal;
            diagnostics.scanned_matchups = gpu_stats.scanned_matchups;
            diagnostics.backend_probe_elapsed = gpu_stats.prepare_elapsed;
            diagnostics.halting_filter_elapsed = gpu_stats.execution_elapsed;
            diagnostics.metal_batches_submitted = gpu_stats.batches_submitted;
            diagnostics.metal_policy_source = Some(gpu_stats.policy_source);
            diagnostics.metal_matches_per_batch = Some(gpu_stats.matches_per_batch);
            diagnostics.metal_inflight_batches = Some(gpu_stats.inflight_batches);
            diagnostics.metal_policy_cache_key = gpu_stats.policy_cache_key;
            diagnostics.metal_policy_cache_path = gpu_stats.policy_cache_path;
            Ok(Some(halting_keep))
        }
        Ok(None) => {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_decline_reason = metal_batch_decline_reason(
                config,
                &config.strategies,
                schedule_len,
            )
            .or_else(|| {
                Some("Metal batch evaluator declined this TM family preparation workload.".into())
            });
            if strict_metal && config.engine.accelerator.requires_metal() {
                let decline_msg = diagnostics.metal_decline_reason.as_deref().unwrap_or(
                    "TM family preparation is not supported by the active Metal backend",
                );
                return Err(format!(
                    "Metal accelerator was requested, but {decline_msg}."
                ));
            }
            Ok(None)
        }
        Err(gpu_error) => {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_error = Some(gpu_error.clone());
            if strict_metal && config.engine.accelerator.requires_metal() {
                return Err(format!(
                    "Metal accelerator unavailable during TM family preparation: {gpu_error}"
                ));
            }
            Ok(None)
        }
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
            metal_batch_decline_reason(config, &config.strategies, schedule_len)
        {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_decline_reason = Some(decline_reason.clone());
            if strict_metal && config.engine.accelerator.requires_metal() {
                return Err(format!(
                    "Metal accelerator was requested, but {decline_reason}."
                ));
            }
        } else {
            let maybe_probe = dispatch_metal_tm_probe(config, diagnostics, probe_started);
            if let Some(probe_result) = maybe_probe {
                let applied = apply_metal_probe_result(
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
    }

    let filter_started = Instant::now();
    let (keep, notebook_stats) = notebook_tm_family_halting_mask(config);
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
    let evaluate_matchup = |matchup: &Matchup| {
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
                        mark_tm_halting_selection(&mut local_keep, &tm_mask, &matchup, &outcome);
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
                mark_tm_halting_selection(&mut keep, &tm_mask, &matchup, &outcome);
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
        .expect(
            "TM halting selection should fall back to the CPU path when strict Metal is not required",
        )
}
