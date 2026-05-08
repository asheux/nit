use super::super::schedule::{matches_per_repetition, SchedulePlan};
use super::super::types::{run_with_parallelism, Parallelism};
use crate::config::{NormalizedConfig, StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::{run_one_sided_tm, symbol_to_action, InputSuffix, TmTransition};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

#[derive(Copy, Clone)]
pub(super) struct NotebookTmSpec<'a> {
    pub(super) symbols: u8,
    pub(super) start_state: u16,
    pub(super) blank: u8,
    pub(super) max_steps_per_round: u32,
    pub(super) transitions: &'a [TmTransition],
}

#[derive(Copy, Clone)]
struct ActionResult {
    action: Action,
    halted: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FamilyStats {
    pub(super) scanned_matchups: usize,
    pub(super) tm_cache_hits: u64,
    pub(super) tm_cache_misses: u64,
    pub(super) tm_evaluations: u64,
    pub(super) tm_steps: u64,
}

#[derive(Default)]
struct EvalCounters {
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    tm_evaluations: AtomicU64,
    tm_steps: AtomicU64,
}

struct EvaluationCache {
    per_strategy: Vec<Mutex<HashMap<Vec<u8>, ActionResult>>>,
    counters: EvalCounters,
}

impl EvaluationCache {
    fn new(strategy_count: usize) -> Self {
        Self {
            per_strategy: (0..strategy_count)
                .map(|_| Mutex::new(HashMap::new()))
                .collect(),
            counters: EvalCounters::default(),
        }
    }

    fn lookup(&self, strategy_idx: usize, input_digits: &[u8]) -> Option<ActionResult> {
        self.per_strategy
            .get(strategy_idx)?
            .lock()
            .expect("TM cache lock poisoned")
            .get(input_digits)
            .copied()
    }

    fn store(&self, strategy_idx: usize, input_digits: Vec<u8>, result: ActionResult) {
        let Some(bucket) = self.per_strategy.get(strategy_idx) else {
            return;
        };
        bucket
            .lock()
            .expect("TM cache lock poisoned")
            .entry(input_digits)
            .or_insert(result);
    }

    fn snapshot(&self) -> FamilyStats {
        FamilyStats {
            scanned_matchups: 0,
            tm_cache_hits: self.counters.cache_hits.load(AtomicOrdering::Relaxed),
            tm_cache_misses: self.counters.cache_misses.load(AtomicOrdering::Relaxed),
            tm_evaluations: self.counters.tm_evaluations.load(AtomicOrdering::Relaxed),
            tm_steps: self.counters.tm_steps.load(AtomicOrdering::Relaxed),
        }
    }
}

pub(super) fn extract_tm_spec(spec: &StrategySpec) -> Option<NotebookTmSpec<'_>> {
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

fn cached_action(
    strategy_idx: usize,
    spec: NotebookTmSpec<'_>,
    input: &InputSuffix,
    first_round: bool,
    cache: &EvaluationCache,
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
    let result = ActionResult {
        action,
        halted: run.halted,
    };
    cache.store(strategy_idx, input_digits, result);
    (result.action, result.halted)
}

fn history_bit(action: Action, halted: bool) -> u8 {
    u8::from(halted && matches!(action, Action::Defect))
}

fn matchup_halts_all_rounds(
    a_idx: usize,
    b_idx: usize,
    tm_specs: &[NotebookTmSpec<'_>],
    rounds: u32,
    cache: &EvaluationCache,
) -> (bool, bool) {
    let a_tm = tm_specs[a_idx];
    let b_tm = tm_specs[b_idx];
    let mut a_input = InputSuffix::new(a_tm.symbols, a_tm.max_steps_per_round as usize + 1);
    let mut b_input = InputSuffix::new(b_tm.symbols, b_tm.max_steps_per_round as usize + 1);
    let mut a_keep = true;
    let mut b_keep = true;

    for round in 0..rounds {
        let first_round = round == 0;
        let (a_action, a_halted) = cached_action(a_idx, a_tm, &a_input, first_round, cache);
        let (b_action, b_halted) = cached_action(b_idx, b_tm, &b_input, first_round, cache);
        a_keep &= a_halted;
        b_keep &= b_halted;

        let a_bit = history_bit(a_action, a_halted);
        let b_bit = history_bit(b_action, b_halted);
        a_input.push_pair_bits(a_bit, b_bit);
        b_input.push_pair_bits(a_bit, b_bit);

        if !a_keep && !b_keep {
            break;
        }
    }

    (a_keep, b_keep)
}

pub(super) fn family_halting_mask(config: &NormalizedConfig) -> (Vec<bool>, FamilyStats) {
    let strategy_count = config.strategies.len();
    let mut keep = vec![true; strategy_count];
    let schedule = SchedulePlan::new(strategy_count, config.repetitions, config.self_play);
    if schedule.is_empty() {
        return (keep, FamilyStats::default());
    }
    let tm_specs = config
        .strategies
        .iter()
        .map(|spec| extract_tm_spec(spec).expect("TM roster should only contain TM strategies"))
        .collect::<Vec<_>>();
    // TM-vs-TM halting outcomes are deterministic, so one repetition's scan
    // covers every repetition. Scan once, reuse the keep-mask everywhere.
    let scanned_matchups = matches_per_repetition(strategy_count, config.self_play).unwrap_or(0);
    if scanned_matchups == 0 {
        return (keep, FamilyStats::default());
    }
    let cache = Arc::new(EvaluationCache::new(strategy_count));

    let evaluate_matchup = |match_id: usize, keep: &mut [bool]| {
        let matchup = schedule
            .matchup(match_id)
            .expect("matchup should exist for in-range id");
        let (a_keep, b_keep) = matchup_halts_all_rounds(
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
