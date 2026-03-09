use crate::config::{
    AcceleratorMode, NormalizedConfig, ParallelismConfig, ParallelismMode, ScoreAggregation,
    StrategySpec, StrategySpecKind,
};
use crate::events::{EventWriter, GameEvent};
use crate::fast_eval::{evaluate_match, CycleMetadata, FastStrategyModel};
use crate::game::{payoffs_with_timeouts, Action, Outcome};
use crate::history::History;
use crate::history_log::{HistoryWriter, MatchHistory};
use crate::output::{
    DominanceEdge, PairwiseResult, RunSummary, RuntimeAcceleratorStats, StrategyDefinition,
    StrategyResult, TournamentResults,
};
use crate::strategy::{CaStrategy, FsmStrategy, OneSidedTmStrategy, Strategy, TmRunStats};
use nit_metal::{BatchPayload, BatchRequest, CaBatch, EvalCommon, FsmBatch, MatchPair, TmBatch};
use nit_utils::hashing::{stable_hash_bytes, XorShift64};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::Sender;

#[derive(Clone, Debug)]
pub struct TournamentProgress {
    pub match_index: usize,
    pub total_matches: usize,
    pub round: u32,
    pub rounds: u32,
    pub a: String,
    pub b: String,
    pub total_payoff_a: i64,
    pub total_payoff_b: i64,
    pub last_action_a: Option<Action>,
    pub last_action_b: Option<Action>,
    pub last_payoff_a: Option<i32>,
    pub last_payoff_b: Option<i32>,
    pub last_halted_a: Option<bool>,
    pub last_halted_b: Option<bool>,
    pub last_outcome: Option<Outcome>,
    pub runtime: RuntimeAcceleratorStats,
}

#[derive(Clone, Debug)]
pub struct MatchSnapshot {
    pub match_index: usize,
    pub total_matches: usize,
    pub round: u32,
    pub rounds: u32,
    pub a: String,
    pub b: String,
    pub a_score: i64,
    pub b_score: i64,
    pub outcomes: String,
    pub payoffs: Vec<[i32; 2]>,
    pub a_halted: String,
    pub b_halted: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchHistoryPreview {
    pub match_index: usize,
    pub total_matches: usize,
    pub a: String,
    pub b: String,
    pub rounds_total: u32,
    #[serde(alias = "outcomes_prefix")]
    pub outcomes: String,
}

impl MatchHistoryPreview {
    pub const DISPLAY_ROUND_CAP: usize = 500;

    pub fn preview_rounds(&self) -> usize {
        self.outcomes.len().min(Self::DISPLAY_ROUND_CAP)
    }

    pub fn preview_outcomes(&self) -> &str {
        let end = self.preview_rounds();
        self.outcomes.get(..end).unwrap_or(self.outcomes.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct MatchResult {
    pub a_idx: usize,
    pub b_idx: usize,
    pub rounds: u32,
    pub a_total: i64,
    pub b_total: i64,
    pub a_adjusted_total: f64,
    pub b_adjusted_total: f64,
    pub repetition: u32,
    pub match_id: usize,
}

#[derive(Copy, Clone, Debug)]
enum MatchRole {
    A,
    B,
}

impl MatchRole {
    fn label(self) -> &'static str {
        match self {
            MatchRole::A => "A",
            MatchRole::B => "B",
        }
    }
}

#[derive(Clone, Debug)]
struct SeedDeriver {
    run_seed: u64,
    noise_base: u64,
}

impl SeedDeriver {
    fn new(run_seed: u64) -> Self {
        let noise_base = stable_hash_bytes(format!("{run_seed}:noise").as_bytes());
        Self {
            run_seed,
            noise_base,
        }
    }

    // Base seed per strategy role; per-match seeds derive from this plus match_id/repetition.
    fn base_strategy_seed(&self, role: MatchRole, strategy_id: &str) -> u64 {
        stable_hash_bytes(format!("{}:{}:{}", self.run_seed, role.label(), strategy_id).as_bytes())
    }

    fn strategy_seed(
        &self,
        match_id: usize,
        repetition: u32,
        role: MatchRole,
        strategy_id: &str,
    ) -> u64 {
        let base = self.base_strategy_seed(role, strategy_id);
        stable_hash_bytes(format!("{base}:{match_id}:{repetition}").as_bytes())
    }

    fn noise_seed(&self, match_id: usize, repetition: u32) -> u64 {
        stable_hash_bytes(format!("{}:{match_id}:{repetition}", self.noise_base).as_bytes())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Parallelism {
    Auto,
    Off,
    Threads(usize),
}

impl Parallelism {
    pub fn from_config(config: &ParallelismConfig) -> Self {
        match config {
            ParallelismConfig::Mode(mode) => match mode {
                ParallelismMode::Auto => Parallelism::Auto,
                ParallelismMode::Off => Parallelism::Off,
            },
            ParallelismConfig::Threads { threads } => Parallelism::Threads(*threads),
        }
    }
}

pub struct TournamentRunner {
    config: NormalizedConfig,
    seed: u64,
    schedule: SchedulePlan,
    match_index: usize,
    current: Option<MatchSession>,
    results: TournamentAccumulator,
    strategies: Vec<StrategySpec>,
    definitions: Vec<StrategyDefinition>,
    seed_deriver: SeedDeriver,
    fast_models: Vec<Option<FastStrategyModel>>,
    event_writer: Option<EventWriter>,
    history_writer: Option<HistoryWriter>,
    last_round: Option<RoundSnapshot>,
    last_progress: Option<TournamentProgress>,
    runtime: RuntimeAcceleratorStats,
    collect_match_history_previews: bool,
    completed_history_previews: Vec<MatchHistoryPreview>,
}

#[derive(Clone, Debug)]
struct Matchup {
    match_id: usize,
    a_idx: usize,
    b_idx: usize,
    repetition: u32,
}

#[derive(Clone, Debug)]
struct SchedulePlan {
    strategy_count: usize,
    repetitions: u32,
    self_play: bool,
    total_matches: usize,
}

impl SchedulePlan {
    fn new(strategy_count: usize, repetitions: u32, self_play: bool) -> Self {
        let total_matches = total_schedule_matches(strategy_count, repetitions, self_play)
            .expect("tournament schedule size overflow");
        Self {
            strategy_count,
            repetitions,
            self_play,
            total_matches,
        }
    }

    fn len(&self) -> usize {
        self.total_matches
    }

    fn is_empty(&self) -> bool {
        self.total_matches == 0
    }

    fn matchup(&self, match_id: usize) -> Option<Matchup> {
        if match_id >= self.total_matches || self.strategy_count == 0 || self.repetitions == 0 {
            return None;
        }
        let matches_per_repetition =
            matches_per_repetition(self.strategy_count, self.self_play).expect("schedule size");
        let repetition = match_id / matches_per_repetition;
        let offset = match_id % matches_per_repetition;
        let (a_idx, b_idx) = if self.self_play {
            (offset / self.strategy_count, offset % self.strategy_count)
        } else {
            let stride = self.strategy_count.saturating_sub(1);
            let a_idx = offset / stride;
            let b_offset = offset % stride;
            let b_idx = if b_offset >= a_idx {
                b_offset + 1
            } else {
                b_offset
            };
            (a_idx, b_idx)
        };
        Some(Matchup {
            match_id,
            a_idx,
            b_idx,
            repetition: repetition as u32,
        })
    }

    fn matchups(&self, start: usize, count: usize) -> Vec<Matchup> {
        let end = start.saturating_add(count).min(self.total_matches);
        (start..end)
            .filter_map(|match_id| self.matchup(match_id))
            .collect()
    }
}

struct MatchSession {
    matchup: Matchup,
    history: History,
    a_strategy: Box<dyn Strategy>,
    b_strategy: Box<dyn Strategy>,
    noise_rng: XorShift64,
    history_actions_a: String,
    history_actions_b: String,
    history_halted_a: String,
    history_halted_b: String,
    history_scores: String,
    history_payoffs: Vec<[i32; 2]>,
    round: u32,
    rounds_total: u32,
    a_total: i64,
    b_total: i64,
    a_crashed: bool,
    b_crashed: bool,
    record_history: bool,
    record_trace: bool,
}

#[derive(Clone, Debug)]
struct RoundSnapshot {
    a_action: Action,
    b_action: Action,
    a_halted: bool,
    b_halted: bool,
    a_payoff: i32,
    b_payoff: i32,
}

struct RoundOutcome {
    snapshot: RoundSnapshot,
    a_crash_now: bool,
    b_crash_now: bool,
}

#[derive(Clone, Debug)]
struct StrategyStats {
    total: i64,
    adjusted_total: f64,
    score_samples: u64,
    matches: u32,
    wins: u32,
    losses: u32,
    draws: u32,
    crash_count: u32,
    crashed: bool,
    tm_stats: Option<TmRunStats>,
}

#[derive(Clone, Debug, Default)]
struct PairStats {
    a_total: i64,
    b_total: i64,
    a_adjusted_total: f64,
    b_adjusted_total: f64,
    a_wins: u32,
    b_wins: u32,
    draws: u32,
}

struct TournamentAccumulator {
    strategies: Vec<StrategyStats>,
    pairwise: Option<Vec<Vec<PairStats>>>,
    use_adjusted: bool,
    score_aggregation: ScoreAggregation,
}

const METAL_BATCH_MATCHES: usize = 16_384;

fn tm_metrics_from_stats(stats: &TmRunStats) -> crate::output::TmDerivedMetrics {
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

fn adjusted_total_for_match(
    raw_total: i64,
    spec: &StrategySpec,
    rounds: u32,
    tm_stats: Option<&TmRunStats>,
    cost: &crate::config::ComplexityCostConfig,
) -> f64 {
    if !cost.enabled {
        return raw_total as f64;
    }
    let mut penalty = 0.0;
    match &spec.kind {
        StrategySpecKind::OneSidedTm { .. } => {
            if cost.tm_step_cost != 0.0 {
                let steps = tm_stats.map(|stats| stats.steps as f64).unwrap_or(0.0);
                penalty += cost.tm_step_cost * steps;
            }
        }
        StrategySpecKind::Fsm {
            num_states,
            outputs,
            ..
        } => {
            if cost.fsm_state_cost != 0.0 {
                let states = if *num_states > 0 {
                    *num_states
                } else {
                    outputs.len()
                };
                penalty += cost.fsm_state_cost * states as f64 * rounds as f64;
            }
        }
        StrategySpecKind::Ca { .. } => {}
    }
    raw_total as f64 - penalty
}

fn timeout_extrema(payoff: crate::game::PayoffMatrix) -> (i32, i32) {
    payoff.min_max()
}

fn move_dir_code(dir: crate::strategy::TmMove) -> u32 {
    match dir {
        crate::strategy::TmMove::Left => 0,
        crate::strategy::TmMove::Right => 1,
        crate::strategy::TmMove::Stay => 2,
    }
}

fn build_metal_batch_payload(strategies: &[StrategySpec]) -> Option<BatchPayload> {
    let first = strategies.first()?;
    match &first.kind {
        StrategySpecKind::Fsm { .. } => build_metal_fsm_payload(strategies).map(BatchPayload::Fsm),
        StrategySpecKind::Ca { .. } => build_metal_ca_payload(strategies).map(BatchPayload::Ca),
        StrategySpecKind::OneSidedTm { .. } => {
            build_metal_tm_payload(strategies).map(BatchPayload::Tm)
        }
    }
}

fn build_metal_fsm_payload(strategies: &[StrategySpec]) -> Option<FsmBatch> {
    let mut starts = Vec::with_capacity(strategies.len());
    let mut outputs = Vec::new();
    let mut transitions = Vec::new();
    let mut expected_states = None;
    let mut expected_alphabet = None;

    for spec in strategies {
        let StrategySpecKind::Fsm {
            num_states,
            start_state,
            outputs: state_outputs,
            input_mode,
            transitions: table,
            ..
        } = &spec.kind
        else {
            return None;
        };
        if !matches!(
            input_mode.unwrap_or(crate::strategy::InputMode::OpponentLastAction),
            crate::strategy::InputMode::OpponentLastAction
        ) {
            return None;
        }
        let states = (*num_states).max(state_outputs.len());
        let alphabet = table.first().map(|row| row.len()).unwrap_or(0);
        if alphabet != 2 || states == 0 || table.len() != states || *start_state >= states {
            return None;
        }
        if table.iter().any(|row| row.len() != alphabet) {
            return None;
        }
        match expected_states {
            Some(value) if value != states => return None,
            None => expected_states = Some(states),
            _ => {}
        }
        match expected_alphabet {
            Some(value) if value != alphabet => return None,
            None => expected_alphabet = Some(alphabet),
            _ => {}
        }
        starts.push(*start_state as u32);
        outputs.extend(state_outputs.iter().map(|action| match action {
            Action::Cooperate => 0u32,
            Action::Defect => 1u32,
        }));
        outputs.resize(
            outputs.len() + states.saturating_sub(state_outputs.len()),
            0,
        );
        for row in table {
            for &next in row {
                if next >= states {
                    return None;
                }
                transitions.push(next as u32);
            }
        }
    }

    Some(FsmBatch {
        states: expected_states? as u32,
        alphabet: expected_alphabet? as u32,
        starts,
        outputs,
        transitions,
    })
}

fn build_metal_ca_payload(strategies: &[StrategySpec]) -> Option<CaBatch> {
    let mut symbols = None;
    let mut two_r = None;
    let mut steps = None;
    let mut rule_tables = Vec::new();
    let mut rule_table_len = None;

    for spec in strategies {
        let StrategySpecKind::Ca { n, k, r, t } = &spec.kind else {
            return None;
        };
        let derived_two_r = (*r * 2.0).round() as u32;
        if ((*r * 2.0) - derived_two_r as f32).abs() > 0.0001 {
            return None;
        }
        match symbols {
            Some(value) if value != *k as u32 => return None,
            None => symbols = Some(*k as u32),
            _ => {}
        }
        match two_r {
            Some(value) if value != derived_two_r => return None,
            None => two_r = Some(derived_two_r),
            _ => {}
        }
        match steps {
            Some(value) if value != *t => return None,
            None => steps = Some(*t),
            _ => {}
        }
        let table = crate::strategy::decode_ca_rule_table(*n, *k, derived_two_r);
        match rule_table_len {
            Some(value) if value != table.len() as u32 => return None,
            None => rule_table_len = Some(table.len() as u32),
            _ => {}
        }
        rule_tables.extend(table.into_iter().map(u32::from));
    }

    Some(CaBatch {
        symbols: symbols?,
        two_r: two_r?,
        steps: steps?,
        rule_table_len: rule_table_len?,
        rule_tables,
    })
}

fn build_metal_tm_payload(strategies: &[StrategySpec]) -> Option<TmBatch> {
    let mut states = None;
    let mut symbols = None;
    let mut blank = None;
    let mut max_steps = None;
    let mut start_states = Vec::with_capacity(strategies.len());
    let mut transitions = Vec::new();

    for spec in strategies {
        let StrategySpecKind::OneSidedTm {
            states: tm_states,
            symbols: tm_symbols,
            start_state,
            blank: tm_blank,
            max_steps_per_round,
            transitions: tm_transitions,
            ..
        } = &spec.kind
        else {
            return None;
        };
        match states {
            Some(value) if value != *tm_states as u32 => return None,
            None => states = Some(*tm_states as u32),
            _ => {}
        }
        match symbols {
            Some(value) if value != *tm_symbols as u32 => return None,
            None => symbols = Some(*tm_symbols as u32),
            _ => {}
        }
        match blank {
            Some(value) if value != *tm_blank as u32 => return None,
            None => blank = Some(*tm_blank as u32),
            _ => {}
        }
        match max_steps {
            Some(value) if value != *max_steps_per_round => return None,
            None => max_steps = Some(*max_steps_per_round),
            _ => {}
        }
        if tm_transitions.len() != (*tm_states as usize).saturating_mul(*tm_symbols as usize) {
            return None;
        }
        if *start_state > *tm_states
            || tm_transitions
                .iter()
                .any(|trans| trans.write >= *tm_symbols || trans.next > *tm_states)
        {
            return None;
        }
        start_states.push(*start_state as u32);
        transitions.extend(
            tm_transitions
                .iter()
                .map(|trans| nit_metal::TmTransitionPacked {
                    write: u32::from(trans.write),
                    move_dir: move_dir_code(trans.move_dir),
                    next: u32::from(trans.next),
                }),
        );
    }

    Some(TmBatch {
        states: states?,
        symbols: symbols?,
        blank: blank?,
        max_steps: max_steps?,
        start_states,
        transitions,
    })
}

fn metal_batch_decline_reason(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchup_count: usize,
) -> Option<String> {
    if matchup_count == 0 {
        return Some("no matchups to evaluate".into());
    }
    if !config.engine.fast_eval {
        return Some("`engine.fast_eval = false` disables Metal batch evaluation".into());
    }
    if !config.engine.accelerator.allows_metal() {
        return Some("accelerator mode is set to CPU".into());
    }
    if config.noise != 0.0 {
        return Some("non-zero noise disables Metal batch evaluation".into());
    }
    if strategies.is_empty() {
        return None;
    }
    if strategies
        .iter()
        .all(|spec| matches!(spec.kind, StrategySpecKind::OneSidedTm { .. }))
        && config.engine.complexity_cost.enabled
        && config.engine.complexity_cost.tm_step_cost != 0.0
    {
        return Some("TM complexity penalties are not supported on the Metal path".into());
    }
    let Some(payload) = build_metal_batch_payload(strategies) else {
        return Some(
            "Metal batch evaluation requires a homogeneous FSM, CA, or TM roster with shared structural parameters."
                .into(),
        );
    };
    match payload {
        BatchPayload::Ca(batch)
            if batch.two_r.saturating_mul(batch.steps).saturating_add(1)
                > nit_metal::CA_MAX_WINDOW =>
        {
            Some(format!(
                "CA window {} exceeds Metal limit {}",
                batch.two_r.saturating_mul(batch.steps).saturating_add(1),
                nit_metal::CA_MAX_WINDOW
            ))
        }
        BatchPayload::Tm(batch) if batch.max_steps.saturating_add(1) > nit_metal::TM_MAX_WIDTH => {
            Some(format!(
                "TM `max_steps_per_round = {}` exceeds Metal limit {}",
                batch.max_steps,
                nit_metal::TM_MAX_WIDTH.saturating_sub(1)
            ))
        }
        _ => None,
    }
}

fn try_metal_batch_outcomes(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchups: &[Matchup],
) -> Result<Option<Vec<MatchOutcome>>, String> {
    if matchups.is_empty()
        || !config.engine.fast_eval
        || !config.engine.accelerator.allows_metal()
        || config.noise != 0.0
    {
        return Ok(None);
    }
    if strategies.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if strategies
        .iter()
        .all(|spec| matches!(spec.kind, StrategySpecKind::OneSidedTm { .. }))
        && config.engine.complexity_cost.enabled
        && config.engine.complexity_cost.tm_step_cost != 0.0
    {
        return Ok(None);
    }
    let Some(payload) = build_metal_batch_payload(strategies) else {
        return Ok(None);
    };
    let (timeout_lose, timeout_win) = timeout_extrema(config.payoff);
    let request = BatchRequest {
        common: EvalCommon {
            rounds: config.rounds,
            payoff: config.payoff.matrix,
            timeout_lose,
            timeout_win,
            pairs: matchups
                .iter()
                .map(|matchup| MatchPair {
                    a_idx: matchup.a_idx as u32,
                    b_idx: matchup.b_idx as u32,
                })
                .collect(),
        },
        payload,
    };
    let Some(scores) = nit_metal::try_evaluate_batch(&request)? else {
        return Ok(None);
    };
    let cost = &config.engine.complexity_cost;
    let outcomes = matchups
        .iter()
        .zip(scores.into_iter())
        .map(|(matchup, score)| {
            let a_spec = &strategies[matchup.a_idx];
            let b_spec = &strategies[matchup.b_idx];
            MatchOutcome {
                result: MatchResult {
                    a_idx: matchup.a_idx,
                    b_idx: matchup.b_idx,
                    rounds: config.rounds,
                    a_total: score.a_total,
                    b_total: score.b_total,
                    a_adjusted_total: adjusted_total_for_match(
                        score.a_total,
                        a_spec,
                        config.rounds,
                        None,
                        cost,
                    ),
                    b_adjusted_total: adjusted_total_for_match(
                        score.b_total,
                        b_spec,
                        config.rounds,
                        None,
                        cost,
                    ),
                    repetition: matchup.repetition,
                    match_id: matchup.match_id,
                },
                a_crashed: false,
                b_crashed: false,
                a_tm_stats: None,
                b_tm_stats: None,
                last_round: None,
            }
        })
        .collect();
    Ok(Some(outcomes))
}

pub fn accelerator_preflight(config: &NormalizedConfig) -> Result<(), String> {
    if !config.engine.accelerator.requires_metal() {
        return Ok(());
    }
    if !config.engine.fast_eval {
        return Err("Metal accelerator requires `engine.fast_eval = true`.".into());
    }
    if config.noise != 0.0 {
        return Err("Metal accelerator requires `noise = 0.0`.".into());
    }
    if config.strategies.is_empty() {
        return Ok(());
    }
    if config
        .strategies
        .iter()
        .all(|spec| matches!(spec.kind, StrategySpecKind::OneSidedTm { .. }))
        && config.engine.complexity_cost.enabled
        && config.engine.complexity_cost.tm_step_cost != 0.0
    {
        return Err(
            "Metal accelerator does not support TM complexity penalties; disable `engine.complexity_cost.tm_step_cost` or use `accelerator = \"auto\"`."
                .into(),
        );
    }

    let payload = build_metal_batch_payload(&config.strategies).ok_or_else(|| {
        "Metal accelerator requires a homogeneous FSM, CA, or TM roster that the Metal batch evaluator can encode."
            .to_string()
    })?;

    match &payload {
        BatchPayload::Ca(batch)
            if batch.two_r.saturating_mul(batch.steps).saturating_add(1)
                > nit_metal::CA_MAX_WINDOW =>
        {
            return Err(format!(
                "Metal accelerator supports CA windows up to {} cells; this run needs {}.",
                nit_metal::CA_MAX_WINDOW,
                batch.two_r.saturating_mul(batch.steps).saturating_add(1)
            ));
        }
        BatchPayload::Tm(batch) if batch.max_steps.saturating_add(1) > nit_metal::TM_MAX_WIDTH => {
            return Err(format!(
                "Metal accelerator supports TM `max_steps_per_round <= {}`; this run uses {}.",
                nit_metal::TM_MAX_WIDTH.saturating_sub(1),
                batch.max_steps
            ));
        }
        _ => {}
    }

    let request = BatchRequest {
        common: EvalCommon {
            rounds: config.rounds,
            payoff: config.payoff.matrix,
            timeout_lose: timeout_extrema(config.payoff).0,
            timeout_win: timeout_extrema(config.payoff).1,
            pairs: vec![MatchPair { a_idx: 0, b_idx: 0 }],
        },
        payload,
    };
    match nit_metal::try_evaluate_batch(&request) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(
            "Metal accelerator was requested, but this run is not supported by the active Metal backend."
                .into(),
        ),
        Err(err) => Err(format!("Metal accelerator unavailable: {err}")),
    }
}

fn try_metal_batch_outcomes_chunked(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchups: &[Matchup],
) -> Result<Option<(Vec<MatchOutcome>, usize)>, String> {
    if matchups.is_empty() {
        return Ok(Some((Vec::new(), 0)));
    }
    let mut outcomes = Vec::with_capacity(matchups.len());
    let mut batches = 0usize;
    for chunk in matchups.chunks(METAL_BATCH_MATCHES) {
        let Some(mut chunk_outcomes) = try_metal_batch_outcomes(config, strategies, chunk)? else {
            return Ok(None);
        };
        batches += 1;
        outcomes.append(&mut chunk_outcomes);
    }
    Ok(Some((outcomes, batches)))
}

#[cfg(test)]
pub(crate) fn metal_batch_totals_for_test(
    config: &NormalizedConfig,
    pairs: &[(usize, usize)],
) -> Result<Option<Vec<(i64, i64)>>, String> {
    let matchups = pairs
        .iter()
        .enumerate()
        .map(|(match_id, (a_idx, b_idx))| Matchup {
            match_id,
            a_idx: *a_idx,
            b_idx: *b_idx,
            repetition: 0,
        })
        .collect::<Vec<_>>();
    Ok(
        try_metal_batch_outcomes(config, &config.strategies, &matchups)?.map(|outcomes| {
            outcomes
                .into_iter()
                .map(|outcome| (outcome.result.a_total, outcome.result.b_total))
                .collect()
        }),
    )
}

fn compare_scores(a: f64, b: f64) -> Ordering {
    let diff = (a - b).abs();
    if diff < 1e-9 {
        Ordering::Equal
    } else if a > b {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

impl TournamentRunner {
    pub fn new(mut config: NormalizedConfig) -> Self {
        let seed = config.seed.unwrap_or(0);
        config.seed = Some(seed);
        let schedule = SchedulePlan::new(
            config.strategies.len(),
            config.repetitions,
            config.self_play,
        );
        let seed_deriver = SeedDeriver::new(seed);
        let definitions = build_strategy_definitions(&config.strategies, &seed_deriver);
        let fast_models = config
            .strategies
            .iter()
            .map(FastStrategyModel::from_spec)
            .collect();
        let use_adjusted = config.engine.complexity_cost.enabled;
        let results = TournamentAccumulator::new(
            config.strategies.len(),
            use_adjusted,
            config.engine.score_aggregation,
            !matches!(config.engine.mode, crate::config::EngineMode::Batch),
        );
        Self {
            config: config.clone(),
            seed,
            schedule,
            match_index: 0,
            current: None,
            results,
            strategies: config.strategies.clone(),
            definitions,
            seed_deriver,
            fast_models,
            event_writer: None,
            history_writer: None,
            last_round: None,
            last_progress: None,
            runtime: RuntimeAcceleratorStats::new(config.engine.accelerator),
            collect_match_history_previews: true,
            completed_history_previews: Vec::new(),
        }
    }

    pub fn with_event_writer(mut self, writer: EventWriter) -> Self {
        self.event_writer = Some(writer);
        self
    }

    pub fn with_history_writer(mut self, writer: HistoryWriter) -> Self {
        self.history_writer = Some(writer);
        self
    }

    pub fn with_match_history_previews(mut self, enabled: bool) -> Self {
        self.collect_match_history_previews = enabled;
        if !enabled {
            self.completed_history_previews.clear();
        }
        self
    }

    pub fn drain_match_history_previews(&mut self) -> Vec<MatchHistoryPreview> {
        std::mem::take(&mut self.completed_history_previews)
    }

    pub fn is_done(&self) -> bool {
        self.match_index >= self.schedule.len() && self.current.is_none()
    }

    pub fn progress(&self) -> Option<TournamentProgress> {
        if self.schedule.is_empty() {
            return Some(TournamentProgress {
                match_index: 0,
                total_matches: 0,
                round: 0,
                rounds: self.config.rounds,
                a: "-".into(),
                b: "-".into(),
                total_payoff_a: 0,
                total_payoff_b: 0,
                last_action_a: None,
                last_action_b: None,
                last_payoff_a: None,
                last_payoff_b: None,
                last_halted_a: None,
                last_halted_b: None,
                last_outcome: None,
                runtime: self.runtime.clone(),
            });
        }
        if let Some(current) = self.current.as_ref() {
            let matchup = &current.matchup;
            let a = strategy_log_id(self.strategies.get(matchup.a_idx)?);
            let b = strategy_log_id(self.strategies.get(matchup.b_idx)?);
            let last_round = if current.round > 0 {
                self.last_round.as_ref()
            } else {
                None
            };
            return Some(TournamentProgress {
                match_index: self.match_index.saturating_add(1),
                total_matches: self.schedule.len().max(1),
                round: current.round,
                rounds: current.rounds_total,
                a,
                b,
                total_payoff_a: current.a_total,
                total_payoff_b: current.b_total,
                last_action_a: last_round.map(|r| r.a_action),
                last_action_b: last_round.map(|r| r.b_action),
                last_payoff_a: last_round.map(|r| r.a_payoff),
                last_payoff_b: last_round.map(|r| r.b_payoff),
                last_halted_a: last_round.map(|r| r.a_halted),
                last_halted_b: last_round.map(|r| r.b_halted),
                last_outcome: last_round.map(|r| Outcome::from_actions(r.a_action, r.b_action)),
                runtime: self.runtime.clone(),
            });
        }
        if matches!(self.config.engine.mode, crate::config::EngineMode::Batch) {
            if let Some(mut progress) = self.last_progress.clone() {
                progress.runtime = self.runtime.clone();
                return Some(progress);
            }
        }
        if let Some(next_match) = self.schedule.matchup(self.match_index) {
            let a = strategy_log_id(self.strategies.get(next_match.a_idx)?);
            let b = strategy_log_id(self.strategies.get(next_match.b_idx)?);
            return Some(TournamentProgress {
                match_index: self.match_index.saturating_add(1),
                total_matches: self.schedule.len().max(1),
                round: 0,
                rounds: self.config.rounds,
                a,
                b,
                total_payoff_a: 0,
                total_payoff_b: 0,
                last_action_a: None,
                last_action_b: None,
                last_payoff_a: None,
                last_payoff_b: None,
                last_halted_a: None,
                last_halted_b: None,
                last_outcome: None,
                runtime: self.runtime.clone(),
            });
        }
        self.last_progress.clone()
    }

    pub fn match_snapshot(&self) -> Option<MatchSnapshot> {
        let current = self.current.as_ref()?;
        let matchup = &current.matchup;
        let a = strategy_log_id(self.strategies.get(matchup.a_idx)?);
        let b = strategy_log_id(self.strategies.get(matchup.b_idx)?);
        Some(MatchSnapshot {
            match_index: self.match_index.saturating_add(1),
            total_matches: self.schedule.len().max(1),
            round: current.round,
            rounds: current.rounds_total,
            a,
            b,
            a_score: current.a_total,
            b_score: current.b_total,
            outcomes: current.history_scores.clone(),
            payoffs: current.history_payoffs.clone(),
            a_halted: current.history_halted_a.clone(),
            b_halted: current.history_halted_b.clone(),
        })
    }

    pub fn step_rounds(&mut self, steps: u32) {
        if self.schedule.is_empty() {
            return;
        }
        if self.match_index == 0 && self.current.is_none() {
            self.emit(GameEvent::TournamentStart {
                timestamp: EventWriter::timestamp(),
                total_matches: self.schedule.len(),
                rounds: self.config.rounds,
            });
        }
        let mut remaining_steps = steps;
        self.try_fast_forward_matches(&mut remaining_steps);
        while remaining_steps > 0 {
            if self.is_done() {
                break;
            }
            if self.current.is_none() {
                if let Some(matchup) = self.schedule.matchup(self.match_index) {
                    let session = MatchSession::new(
                        matchup,
                        &self.config,
                        &self.strategies,
                        &self.seed_deriver,
                        true,
                        true,
                    );
                    self.emit(GameEvent::MatchStart {
                        timestamp: EventWriter::timestamp(),
                        match_id: session.matchup.match_id,
                        match_index: self.match_index + 1,
                        total_matches: self.schedule.len(),
                        a: self.strategies[session.matchup.a_idx].id.clone(),
                        b: self.strategies[session.matchup.b_idx].id.clone(),
                        repetition: session.matchup.repetition + 1,
                    });
                    self.last_progress = Some(TournamentProgress {
                        match_index: self.match_index.saturating_add(1),
                        total_matches: self.schedule.len().max(1),
                        round: 0,
                        rounds: session.rounds_total,
                        a: self.strategies[session.matchup.a_idx].id.clone(),
                        b: self.strategies[session.matchup.b_idx].id.clone(),
                        total_payoff_a: 0,
                        total_payoff_b: 0,
                        last_action_a: None,
                        last_action_b: None,
                        last_payoff_a: None,
                        last_payoff_b: None,
                        last_halted_a: None,
                        last_halted_b: None,
                        last_outcome: None,
                        runtime: self.runtime.clone(),
                    });
                    self.current = Some(session);
                } else {
                    break;
                }
            }

            if let Some(mut session) = self.current.take() {
                let snapshot = self.play_round(&mut session);
                self.last_round = Some(snapshot.clone());
                self.last_progress = Some(TournamentProgress {
                    match_index: self.match_index.saturating_add(1),
                    total_matches: self.schedule.len().max(1),
                    round: session.round,
                    rounds: session.rounds_total,
                    a: strategy_log_id(&self.strategies[session.matchup.a_idx]),
                    b: strategy_log_id(&self.strategies[session.matchup.b_idx]),
                    total_payoff_a: session.a_total,
                    total_payoff_b: session.b_total,
                    last_action_a: Some(snapshot.a_action),
                    last_action_b: Some(snapshot.b_action),
                    last_payoff_a: Some(snapshot.a_payoff),
                    last_payoff_b: Some(snapshot.b_payoff),
                    last_halted_a: Some(snapshot.a_halted),
                    last_halted_b: Some(snapshot.b_halted),
                    last_outcome: Some(Outcome::from_actions(snapshot.a_action, snapshot.b_action)),
                    runtime: self.runtime.clone(),
                });
                if session.round >= session.rounds_total {
                    if self.collect_match_history_previews {
                        self.completed_history_previews.push(MatchHistoryPreview {
                            match_index: self.match_index.saturating_add(1),
                            total_matches: self.schedule.len().max(1),
                            a: strategy_log_id(&self.strategies[session.matchup.a_idx]),
                            b: strategy_log_id(&self.strategies[session.matchup.b_idx]),
                            rounds_total: session.rounds_total,
                            outcomes: session.history_scores.clone(),
                        });
                    }
                    let a_spec = &self.strategies[session.matchup.a_idx];
                    let b_spec = &self.strategies[session.matchup.b_idx];
                    let cost = &self.config.engine.complexity_cost;
                    let a_tm_stats = session.a_strategy.tm_stats();
                    let b_tm_stats = session.b_strategy.tm_stats();
                    let a_adjusted_total = adjusted_total_for_match(
                        session.a_total,
                        a_spec,
                        session.rounds_total,
                        a_tm_stats,
                        cost,
                    );
                    let b_adjusted_total = adjusted_total_for_match(
                        session.b_total,
                        b_spec,
                        session.rounds_total,
                        b_tm_stats,
                        cost,
                    );
                    let result = MatchResult {
                        a_idx: session.matchup.a_idx,
                        b_idx: session.matchup.b_idx,
                        rounds: session.rounds_total,
                        a_total: session.a_total,
                        b_total: session.b_total,
                        a_adjusted_total,
                        b_adjusted_total,
                        repetition: session.matchup.repetition,
                        match_id: session.matchup.match_id,
                    };
                    self.emit(GameEvent::MatchEnd {
                        timestamp: EventWriter::timestamp(),
                        match_id: session.matchup.match_id,
                        match_index: self.match_index + 1,
                        a_total: session.a_total,
                        b_total: session.b_total,
                    });
                    self.emit_history(&session);
                    self.runtime.note_cpu_matches(1);
                    self.record_completed_outcome(MatchOutcome {
                        result,
                        a_crashed: session.a_crashed,
                        b_crashed: session.b_crashed,
                        a_tm_stats: a_tm_stats.cloned(),
                        b_tm_stats: b_tm_stats.cloned(),
                        last_round: self.last_round.clone(),
                    });
                    if self.is_done() {
                        self.emit(GameEvent::TournamentEnd {
                            timestamp: EventWriter::timestamp(),
                        });
                    }
                } else {
                    self.current = Some(session);
                }
            }
            remaining_steps = remaining_steps.saturating_sub(1);
            self.try_fast_forward_matches(&mut remaining_steps);
        }
    }

    pub fn results(&self) -> TournamentResults {
        self.results.finalize(&self.strategies)
    }

    pub fn leaderboard(&self) -> TournamentResults {
        self.results.leaderboard(&self.strategies)
    }

    pub fn definitions(&self) -> &[StrategyDefinition] {
        &self.definitions
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn config(&self) -> &NormalizedConfig {
        &self.config
    }

    pub fn runtime(&self) -> &RuntimeAcceleratorStats {
        &self.runtime
    }

    pub fn completed_matches(&self) -> usize {
        self.match_index
    }

    pub fn total_matches(&self) -> usize {
        self.schedule.len()
    }

    pub fn finish(mut self, timestamp: String, run_id: String, config_text: String) -> RunSummary {
        let event_log = self
            .event_writer
            .take()
            .and_then(|writer| writer.finish().ok())
            .map(|p| p.to_string_lossy().to_string());
        let history_log = self
            .history_writer
            .take()
            .and_then(|writer| writer.finish().ok())
            .map(|p| p.to_string_lossy().to_string());
        let results = self.results();
        RunSummary {
            schema_version: crate::output::RUN_SUMMARY_SCHEMA_VERSION,
            timestamp,
            run_id,
            seed: self.seed,
            config_text,
            config: self.config.clone(),
            paths: crate::output::RunPaths {
                summary: None,
                events: event_log.clone(),
                history: history_log.clone(),
                definitions: None,
                results: None,
                config: None,
                analysis_dir: None,
            },
            strategies: self.definitions.clone(),
            results,
            event_log,
            history_log,
            runtime: self.runtime.clone(),
            run_dir: None,
        }
    }

    fn emit(&mut self, event: GameEvent) {
        if let Some(writer) = self.event_writer.as_mut() {
            if matches!(event, GameEvent::Round { .. }) && !writer.include_rounds() {
                return;
            }
            let _ = writer.write(&event);
        }
    }

    fn emit_history(&mut self, session: &MatchSession) {
        let Some(writer) = self.history_writer.as_mut() else {
            return;
        };
        let a = strategy_log_id(&self.strategies[session.matchup.a_idx]);
        let b = strategy_log_id(&self.strategies[session.matchup.b_idx]);
        let a_moves = session.history_actions_a.clone();
        let b_moves = session.history_actions_b.clone();
        let a_halted = session.history_halted_a.clone();
        let b_halted = session.history_halted_b.clone();
        let include_tm_metrics = self.config.history.include_cycle_metadata;
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
        let record = MatchHistory {
            event: "match_history".into(),
            timestamp: EventWriter::timestamp(),
            match_id: session.matchup.match_id,
            match_index: self.match_index + 1,
            total_matches: self.schedule.len(),
            a,
            b,
            repetition: session.matchup.repetition + 1,
            rounds: session.rounds_total,
            a_moves: a_moves.clone(),
            b_moves: b_moves.clone(),
            a_halted,
            b_halted,
            a_incoming: b_moves,
            b_incoming: a_moves,
            score_idx: session.history_scores.clone(),
            a_score: session.a_total,
            b_score: session.b_total,
            a_initial: session.history_actions_a.chars().next(),
            b_initial: session.history_actions_b.chars().next(),
            cycle: None,
            a_tm_metrics,
            b_tm_metrics,
        };
        let _ = writer.write(&record);
    }

    fn fast_forward_allowed(&self) -> bool {
        self.current.is_none()
            && self.config.engine.fast_eval
            && self.config.noise == 0.0
            && self.event_writer.is_none()
            && self.history_writer.is_none()
            && !self.collect_match_history_previews
    }

    fn try_fast_forward_matches(&mut self, remaining_steps: &mut u32) {
        if !self.fast_forward_allowed() {
            return;
        }
        let rounds_per_match = self.config.rounds.max(1);
        let match_budget = (*remaining_steps / rounds_per_match) as usize;
        if match_budget == 0 {
            return;
        }
        let available = self.schedule.len().saturating_sub(self.match_index);
        let matches_to_run = match_budget.min(available);
        if matches_to_run == 0 {
            return;
        }

        let total_matches = self.schedule.len();
        let matchups = self.schedule.matchups(self.match_index, matches_to_run);
        let config = &self.config;
        let strategies = &self.strategies;
        let seed_deriver = &self.seed_deriver;
        let fast_models = &self.fast_models;
        let run_matchup = |matchup: &Matchup, fast_eval_allowed: bool| {
            let mut emit_event = |_event: GameEvent| {};
            let mut emit_history = |_record: MatchHistory| {};
            run_match_core(
                matchup,
                config,
                strategies,
                seed_deriver,
                Some(fast_models),
                fast_eval_allowed,
                total_matches,
                false,
                false,
                &mut emit_event,
                false,
                &mut emit_history,
                false,
            )
        };
        let (tail_matchup, head_matchups) = matchups
            .split_last()
            .expect("fast-forward batches are non-empty");
        let run_parallel = || {
            head_matchups
                .par_iter()
                .map(|matchup| run_matchup(matchup, true))
                .collect::<Vec<_>>()
        };
        let (mut outcomes, gpu_used) =
            match try_metal_batch_outcomes_chunked(config, strategies, head_matchups) {
                Ok(Some((gpu_outcomes, metal_batches))) => {
                    self.runtime
                        .note_metal_batches(metal_batches, head_matchups.len());
                    (gpu_outcomes, true)
                }
                Ok(None) => {
                    if !head_matchups.is_empty() && self.config.engine.accelerator.allows_metal() {
                        self.runtime.note_metal_fallback_reason(
                            metal_batch_decline_reason(config, strategies, head_matchups.len())
                                .unwrap_or_else(|| {
                                    "Metal batch evaluator declined this workload".into()
                                }),
                        );
                    }
                    let outcomes = match Parallelism::from_config(&self.config.engine.parallelism) {
                        Parallelism::Off => head_matchups
                            .iter()
                            .map(|matchup| run_matchup(matchup, true))
                            .collect(),
                        Parallelism::Threads(threads) if threads > 0 => {
                            let pool = ThreadPoolBuilder::new()
                                .num_threads(threads)
                                .build()
                                .unwrap_or_else(|_| {
                                    ThreadPoolBuilder::new().build().expect("thread pool")
                                });
                            pool.install(run_parallel)
                        }
                        _ => run_parallel(),
                    };
                    (outcomes, false)
                }
                Err(err) => {
                    if !head_matchups.is_empty() && self.config.engine.accelerator.allows_metal() {
                        self.runtime
                            .note_metal_fallback_reason(format!("Metal backend error: {err}"));
                    }
                    let outcomes = match Parallelism::from_config(&self.config.engine.parallelism) {
                        Parallelism::Off => head_matchups
                            .iter()
                            .map(|matchup| run_matchup(matchup, true))
                            .collect(),
                        Parallelism::Threads(threads) if threads > 0 => {
                            let pool = ThreadPoolBuilder::new()
                                .num_threads(threads)
                                .build()
                                .unwrap_or_else(|_| {
                                    ThreadPoolBuilder::new().build().expect("thread pool")
                                });
                            pool.install(run_parallel)
                        }
                        _ => run_parallel(),
                    };
                    (outcomes, false)
                }
            };
        self.runtime.note_cpu_matches(1);
        if !gpu_used {
            self.runtime.note_cpu_matches(head_matchups.len());
        }
        outcomes.push(run_matchup(tail_matchup, false));

        self.last_round = None;
        for outcome in outcomes {
            self.record_completed_outcome(outcome);
        }
        *remaining_steps = remaining_steps
            .saturating_sub((matches_to_run as u32).saturating_mul(rounds_per_match));
        if self.is_done() {
            self.emit(GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }
    }

    fn record_completed_outcome(&mut self, outcome: MatchOutcome) {
        let MatchOutcome {
            result,
            a_crashed,
            b_crashed,
            a_tm_stats,
            b_tm_stats,
            last_round,
        } = outcome;
        let completed_match = self.match_index.saturating_add(1);
        self.last_round = last_round.clone();
        if self
            .last_progress
            .as_ref()
            .map(|progress| (progress.match_index, progress.round))
            != Some((completed_match, result.rounds))
        {
            self.last_progress = Some(TournamentProgress {
                match_index: completed_match,
                total_matches: self.schedule.len().max(1),
                round: result.rounds,
                rounds: result.rounds,
                a: strategy_log_id(&self.strategies[result.a_idx]),
                b: strategy_log_id(&self.strategies[result.b_idx]),
                total_payoff_a: result.a_total,
                total_payoff_b: result.b_total,
                last_action_a: last_round.as_ref().map(|round| round.a_action),
                last_action_b: last_round.as_ref().map(|round| round.b_action),
                last_payoff_a: last_round.as_ref().map(|round| round.a_payoff),
                last_payoff_b: last_round.as_ref().map(|round| round.b_payoff),
                last_halted_a: last_round.as_ref().map(|round| round.a_halted),
                last_halted_b: last_round.as_ref().map(|round| round.b_halted),
                last_outcome: last_round
                    .as_ref()
                    .map(|round| Outcome::from_actions(round.a_action, round.b_action)),
                runtime: self.runtime.clone(),
            });
        }
        if a_crashed {
            self.results.strategies[result.a_idx].crash_count += 1;
            self.results.strategies[result.a_idx].crashed = true;
        }
        if b_crashed {
            self.results.strategies[result.b_idx].crash_count += 1;
            self.results.strategies[result.b_idx].crashed = true;
        }
        self.results
            .apply_match(result, a_crashed, b_crashed, a_tm_stats, b_tm_stats);
        self.match_index += 1;
    }

    fn play_round(&mut self, session: &mut MatchSession) -> RoundSnapshot {
        self.runtime.note_cpu_activity();
        let a_idx = session.matchup.a_idx;
        let b_idx = session.matchup.b_idx;
        let a_id = self.strategies[a_idx].id.clone();
        let b_id = self.strategies[b_idx].id.clone();

        let outcome = play_round_core(session, &self.config);

        if outcome.a_crash_now {
            session.a_crashed = true;
            self.results.strategies[a_idx].crash_count += 1;
            self.results.strategies[a_idx].crashed = true;
            self.emit(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: a_id,
                error: "panic in strategy".into(),
            });
        }
        if outcome.b_crash_now {
            session.b_crashed = true;
            self.results.strategies[b_idx].crash_count += 1;
            self.results.strategies[b_idx].crashed = true;
            self.emit(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: b_id,
                error: "panic in strategy".into(),
            });
        }

        self.emit(GameEvent::Round {
            timestamp: EventWriter::timestamp(),
            match_id: session.matchup.match_id,
            match_index: self.match_index + 1,
            round: session.round,
            a_action: outcome.snapshot.a_action.as_char(),
            b_action: outcome.snapshot.b_action.as_char(),
            a_halted: outcome.snapshot.a_halted,
            b_halted: outcome.snapshot.b_halted,
            a_payoff: outcome.snapshot.a_payoff,
            b_payoff: outcome.snapshot.b_payoff,
        });
        outcome.snapshot
    }
}

fn play_round_core(session: &mut MatchSession, config: &NormalizedConfig) -> RoundOutcome {
    let (a_action, b_action, a_halted, b_halted, a_crash_now, b_crash_now) = {
        let mut a_crash = false;
        let mut b_crash = false;
        let (a_action, a_halted) = if session.a_crashed {
            (Action::Defect, false)
        } else {
            match catch_unwind(AssertUnwindSafe(|| {
                session.a_strategy.next_action(&session.history, true)
            })) {
                Ok(action) => (action, session.a_strategy.last_halted()),
                Err(_) => {
                    a_crash = true;
                    (Action::Defect, false)
                }
            }
        };
        let (b_action, b_halted) = if session.b_crashed {
            (Action::Defect, false)
        } else {
            match catch_unwind(AssertUnwindSafe(|| {
                session.b_strategy.next_action(&session.history, false)
            })) {
                Ok(action) => (action, session.b_strategy.last_halted()),
                Err(_) => {
                    b_crash = true;
                    (Action::Defect, false)
                }
            }
        };
        (a_action, b_action, a_halted, b_halted, a_crash, b_crash)
    };

    if a_crash_now {
        session.a_crashed = true;
    }
    if b_crash_now {
        session.b_crashed = true;
    }

    let a_action = apply_noise(config.noise, a_action, &mut session.noise_rng);
    let b_action = apply_noise(config.noise, b_action, &mut session.noise_rng);
    let (a_payoff, b_payoff) =
        payoffs_with_timeouts(config.payoff, a_action, b_action, a_halted, b_halted);
    let outcome = Outcome::from_actions(a_action, b_action);
    session.a_total += a_payoff as i64;
    session.b_total += b_payoff as i64;
    session.history.push(a_action, b_action);
    if session.record_history || session.record_trace {
        session.history_scores.push(outcome_char(outcome));
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

fn apply_noise(noise: f32, action: Action, rng: &mut XorShift64) -> Action {
    if noise <= 0.0 {
        return action;
    }
    if rng.next_f32() < noise {
        action.flip()
    } else {
        action
    }
}

impl MatchSession {
    fn new(
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
        let mut a_strategy = build_strategy(a_spec, a_seed);
        let mut b_strategy = build_strategy(b_spec, b_seed);
        a_strategy.reset();
        b_strategy.reset();
        let noise_seed = seed_deriver.noise_seed(matchup.match_id, matchup.repetition);
        let record_scores = record_history || record_trace;
        Self {
            matchup,
            history: History::new(max_memory),
            a_strategy,
            b_strategy,
            noise_rng: XorShift64::new(noise_seed),
            history_actions_a: if record_history {
                String::with_capacity(rounds_total as usize)
            } else {
                String::new()
            },
            history_actions_b: if record_history {
                String::with_capacity(rounds_total as usize)
            } else {
                String::new()
            },
            history_halted_a: if record_history {
                String::with_capacity(rounds_total as usize)
            } else {
                String::new()
            },
            history_halted_b: if record_history {
                String::with_capacity(rounds_total as usize)
            } else {
                String::new()
            },
            history_scores: if record_scores {
                String::with_capacity(rounds_total as usize)
            } else {
                String::new()
            },
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

struct MatchOutcome {
    result: MatchResult,
    a_crashed: bool,
    b_crashed: bool,
    a_tm_stats: Option<TmRunStats>,
    b_tm_stats: Option<TmRunStats>,
    last_round: Option<RoundSnapshot>,
}

pub enum KernelRunMode<'a> {
    Sequential {
        event_writer: Option<&'a mut EventWriter>,
        history_writer: Option<&'a mut HistoryWriter>,
    },
    Parallel {
        parallelism: Parallelism,
        // Logs are written via channels; NDJSON line order is nondeterministic.
        // Use match_id/match_index fields to reconstruct ordering.
        event_sender: Option<Sender<GameEvent>>,
        include_rounds: bool,
        history_sender: Option<Sender<MatchHistory>>,
    },
}

pub struct TournamentKernel {
    config: NormalizedConfig,
    seed: u64,
    schedule: SchedulePlan,
    definitions: Vec<StrategyDefinition>,
    seed_deriver: SeedDeriver,
    fast_models: Vec<Option<FastStrategyModel>>,
}

impl TournamentKernel {
    pub fn new(mut config: NormalizedConfig) -> Self {
        let seed = config.seed.unwrap_or(0);
        config.seed = Some(seed);
        let schedule = SchedulePlan::new(
            config.strategies.len(),
            config.repetitions,
            config.self_play,
        );
        let seed_deriver = SeedDeriver::new(seed);
        let definitions = build_strategy_definitions(&config.strategies, &seed_deriver);
        let fast_models = config
            .strategies
            .iter()
            .map(FastStrategyModel::from_spec)
            .collect();
        Self {
            config,
            seed,
            schedule,
            definitions,
            seed_deriver,
            fast_models,
        }
    }

    pub fn run(&self, mode: KernelRunMode<'_>) -> TournamentResults {
        self.run_with_runtime(mode).0
    }

    pub fn run_with_runtime(
        &self,
        mode: KernelRunMode<'_>,
    ) -> (TournamentResults, RuntimeAcceleratorStats) {
        match mode {
            KernelRunMode::Sequential {
                event_writer,
                history_writer,
            } => self.run_sequential(event_writer, history_writer),
            KernelRunMode::Parallel {
                parallelism,
                event_sender,
                include_rounds,
                history_sender,
            } => self.run_parallel(parallelism, event_sender, include_rounds, history_sender),
        }
    }

    pub fn definitions(&self) -> &[StrategyDefinition] {
        &self.definitions
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn config(&self) -> &NormalizedConfig {
        &self.config
    }

    pub fn total_matches(&self) -> usize {
        self.schedule.len()
    }

    fn run_sequential(
        &self,
        mut event_writer: Option<&mut EventWriter>,
        mut history_writer: Option<&mut HistoryWriter>,
    ) -> (TournamentResults, RuntimeAcceleratorStats) {
        let total_matches = self.schedule.len();
        let mut runtime = RuntimeAcceleratorStats::new(self.config.engine.accelerator);
        let mut results = TournamentAccumulator::new(
            self.config.strategies.len(),
            self.config.engine.complexity_cost.enabled,
            self.config.engine.score_aggregation,
            !matches!(self.config.engine.mode, crate::config::EngineMode::Batch),
        );
        if let Some(writer) = event_writer.as_mut() {
            let _ = writer.write(&GameEvent::TournamentStart {
                timestamp: EventWriter::timestamp(),
                total_matches,
                rounds: self.config.rounds,
            });
        }

        let include_rounds = event_writer
            .as_ref()
            .map(|writer| writer.include_rounds())
            .unwrap_or(false);
        let log_events = event_writer.is_some();
        let log_history = history_writer.is_some();

        let fast_eval_allowed = self.config.engine.fast_eval
            && self.config.noise == 0.0
            && !log_history
            && !(log_events && include_rounds);

        if fast_eval_allowed
            && !matches!(self.config.engine.accelerator, AcceleratorMode::Cpu)
            && !log_events
            && !log_history
            && self.schedule.len() > 0
        {
            let probe = self
                .schedule
                .matchups(0, METAL_BATCH_MATCHES.min(self.schedule.len()));
            match try_metal_batch_outcomes(&self.config, &self.config.strategies, &probe) {
                Ok(Some(_)) => {
                    let mut next_match = 0usize;
                    let mut batches = 0usize;
                    while next_match < self.schedule.len() {
                        let count = METAL_BATCH_MATCHES.min(self.schedule.len() - next_match);
                        let matchups = self.schedule.matchups(next_match, count);
                        let outcomes = try_metal_batch_outcomes(
                            &self.config,
                            &self.config.strategies,
                            &matchups,
                        )
                        .expect("metal batch support should remain stable across chunks")
                        .expect("metal batch support should remain stable across chunks");
                        batches += 1;
                        for outcome in outcomes {
                            results.apply_match(
                                outcome.result,
                                outcome.a_crashed,
                                outcome.b_crashed,
                                outcome.a_tm_stats,
                                outcome.b_tm_stats,
                            );
                        }
                        next_match += count;
                    }
                    runtime.note_metal_batches(batches, self.schedule.len());
                    if let Some(writer) = event_writer.as_mut() {
                        let _ = writer.write(&GameEvent::TournamentEnd {
                            timestamp: EventWriter::timestamp(),
                        });
                    }
                    return (results.finalize(&self.config.strategies), runtime);
                }
                Ok(None) => runtime.note_metal_fallback_reason(
                    metal_batch_decline_reason(&self.config, &self.config.strategies, probe.len())
                        .unwrap_or_else(|| "Metal batch evaluator declined the probe".into()),
                ),
                Err(err) => {
                    runtime.note_metal_fallback_reason(format!("Metal backend error: {err}"))
                }
            }
        }

        for match_id in 0..self.schedule.len() {
            let matchup = self
                .schedule
                .matchup(match_id)
                .expect("matchup should exist for in-range id");
            let mut emit_event = |event: GameEvent| {
                if let Some(writer) = event_writer.as_mut() {
                    if matches!(event, GameEvent::Round { .. }) && !include_rounds {
                        return;
                    }
                    let _ = writer.write(&event);
                }
            };
            let mut emit_history = |record: MatchHistory| {
                if let Some(writer) = history_writer.as_mut() {
                    let _ = writer.write(&record);
                }
            };
            let outcome = run_match_core(
                &matchup,
                &self.config,
                &self.config.strategies,
                &self.seed_deriver,
                Some(&self.fast_models),
                fast_eval_allowed,
                total_matches,
                log_events,
                include_rounds,
                &mut emit_event,
                log_history,
                &mut emit_history,
                false,
            );
            results.apply_match(
                outcome.result,
                outcome.a_crashed,
                outcome.b_crashed,
                outcome.a_tm_stats,
                outcome.b_tm_stats,
            );
        }
        runtime.note_cpu_matches(self.schedule.len());

        if let Some(writer) = event_writer.as_mut() {
            let _ = writer.write(&GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }
        (results.finalize(&self.config.strategies), runtime)
    }

    fn run_parallel(
        &self,
        parallelism: Parallelism,
        event_sender: Option<Sender<GameEvent>>,
        include_rounds: bool,
        history_sender: Option<Sender<MatchHistory>>,
    ) -> (TournamentResults, RuntimeAcceleratorStats) {
        let total_matches = self.schedule.len();
        let mut runtime = RuntimeAcceleratorStats::new(self.config.engine.accelerator);
        if let Some(sender) = event_sender.as_ref() {
            let _ = sender.send(GameEvent::TournamentStart {
                timestamp: EventWriter::timestamp(),
                total_matches,
                rounds: self.config.rounds,
            });
        }

        let log_events = event_sender.is_some();
        let log_history = history_sender.is_some();
        let event_sender_for_run = event_sender.clone();
        let history_sender_for_run = history_sender.clone();

        let fast_eval_allowed = self.config.engine.fast_eval
            && self.config.noise == 0.0
            && !log_history
            && !(log_events && include_rounds);

        if fast_eval_allowed
            && !matches!(self.config.engine.accelerator, AcceleratorMode::Cpu)
            && !log_events
            && !log_history
            && self.schedule.len() > 0
        {
            let probe = self
                .schedule
                .matchups(0, METAL_BATCH_MATCHES.min(self.schedule.len()));
            match try_metal_batch_outcomes(&self.config, &self.config.strategies, &probe) {
                Ok(Some(_)) => {
                    let mut all_outcomes = Vec::with_capacity(self.schedule.len());
                    let mut next_match = 0usize;
                    let mut batches = 0usize;
                    while next_match < self.schedule.len() {
                        let count = METAL_BATCH_MATCHES.min(self.schedule.len() - next_match);
                        let matchups = self.schedule.matchups(next_match, count);
                        let mut outcomes = try_metal_batch_outcomes(
                            &self.config,
                            &self.config.strategies,
                            &matchups,
                        )
                        .expect("metal batch support should remain stable across chunks")
                        .expect("metal batch support should remain stable across chunks");
                        batches += 1;
                        all_outcomes.append(&mut outcomes);
                        next_match += count;
                    }
                    runtime.note_metal_batches(batches, self.schedule.len());
                    if let Some(sender) = event_sender.as_ref() {
                        let _ = sender.send(GameEvent::TournamentEnd {
                            timestamp: EventWriter::timestamp(),
                        });
                    }
                    let mut results = TournamentAccumulator::new(
                        self.config.strategies.len(),
                        self.config.engine.complexity_cost.enabled,
                        self.config.engine.score_aggregation,
                        !matches!(self.config.engine.mode, crate::config::EngineMode::Batch),
                    );
                    for outcome in all_outcomes {
                        results.apply_match(
                            outcome.result,
                            outcome.a_crashed,
                            outcome.b_crashed,
                            outcome.a_tm_stats,
                            outcome.b_tm_stats,
                        );
                    }
                    return (results.finalize(&self.config.strategies), runtime);
                }
                Ok(None) => runtime.note_metal_fallback_reason(
                    metal_batch_decline_reason(&self.config, &self.config.strategies, probe.len())
                        .unwrap_or_else(|| "Metal batch evaluator declined the probe".into()),
                ),
                Err(err) => {
                    runtime.note_metal_fallback_reason(format!("Metal backend error: {err}"))
                }
            }
        }

        let run = || {
            (0..self.schedule.len())
                .into_par_iter()
                .map(move |match_id| {
                    let matchup = self
                        .schedule
                        .matchup(match_id)
                        .expect("matchup should exist for in-range id");
                    let event_tx = event_sender_for_run.clone();
                    let history_tx = history_sender_for_run.clone();
                    let mut emit_event = move |event: GameEvent| {
                        if let Some(sender) = event_tx.as_ref() {
                            let _ = sender.send(event);
                        }
                    };
                    let mut emit_history = move |record: MatchHistory| {
                        if let Some(sender) = history_tx.as_ref() {
                            let _ = sender.send(record);
                        }
                    };
                    run_match_core(
                        &matchup,
                        &self.config,
                        &self.config.strategies,
                        &self.seed_deriver,
                        Some(&self.fast_models),
                        fast_eval_allowed,
                        total_matches,
                        log_events,
                        include_rounds,
                        &mut emit_event,
                        log_history,
                        &mut emit_history,
                        false,
                    )
                })
                .collect::<Vec<_>>()
        };

        let outcomes = match parallelism {
            Parallelism::Threads(threads) if threads > 0 => {
                let pool = ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap_or_else(|_| ThreadPoolBuilder::new().build().expect("thread pool"));
                pool.install(run)
            }
            _ => run(),
        };

        if let Some(sender) = event_sender.as_ref() {
            let _ = sender.send(GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }

        let mut results = TournamentAccumulator::new(
            self.config.strategies.len(),
            self.config.engine.complexity_cost.enabled,
            self.config.engine.score_aggregation,
            !matches!(self.config.engine.mode, crate::config::EngineMode::Batch),
        );
        for outcome in outcomes {
            results.apply_match(
                outcome.result,
                outcome.a_crashed,
                outcome.b_crashed,
                outcome.a_tm_stats,
                outcome.b_tm_stats,
            );
        }
        runtime.note_cpu_matches(self.schedule.len());
        (results.finalize(&self.config.strategies), runtime)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_match_core<E, H>(
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
    let owned_ids = if log_events || log_history {
        Some((a_id.clone(), b_id.clone()))
    } else {
        None
    };

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
            let eval = evaluate_match(a_model, b_model, config.rounds, config.payoff, false);
            if log_events {
                emit_event(GameEvent::MatchEnd {
                    timestamp: EventWriter::timestamp(),
                    match_id: matchup.match_id,
                    match_index,
                    a_total: eval.a_total,
                    b_total: eval.b_total,
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
            cycle_meta = evaluate_match(a_model, b_model, config.rounds, config.payoff, true).cycle;
        }
    }

    if log_history {
        let a_moves = session.history_actions_a.clone();
        let b_moves = session.history_actions_b.clone();
        let a_halted = session.history_halted_a.clone();
        let b_halted = session.history_halted_b.clone();
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
            event: "match_history".into(),
            timestamp: EventWriter::timestamp(),
            match_id: matchup.match_id,
            match_index,
            total_matches,
            a: a_owned.clone(),
            b: b_owned.clone(),
            repetition: matchup.repetition + 1,
            rounds: session.rounds_total,
            a_moves: a_moves.clone(),
            b_moves: b_moves.clone(),
            a_halted,
            b_halted,
            a_incoming: b_moves,
            b_incoming: a_moves,
            score_idx: session.history_scores.clone(),
            a_score: session.a_total,
            b_score: session.b_total,
            a_initial: session.history_actions_a.chars().next(),
            b_initial: session.history_actions_b.chars().next(),
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

fn outcome_char(outcome: Outcome) -> char {
    match outcome {
        Outcome::CC => '0',
        Outcome::CD => '1',
        Outcome::DC => '2',
        Outcome::DD => '3',
    }
}

impl TournamentAccumulator {
    fn new(
        n: usize,
        use_adjusted: bool,
        score_aggregation: ScoreAggregation,
        store_pairwise: bool,
    ) -> Self {
        Self {
            strategies: vec![
                StrategyStats {
                    total: 0,
                    adjusted_total: 0.0,
                    score_samples: 0,
                    matches: 0,
                    wins: 0,
                    losses: 0,
                    draws: 0,
                    crash_count: 0,
                    crashed: false,
                    tm_stats: None,
                };
                n
            ],
            pairwise: store_pairwise.then(|| vec![vec![PairStats::default(); n]; n]),
            use_adjusted,
            score_aggregation,
        }
    }

    fn apply_match(
        &mut self,
        result: MatchResult,
        a_crashed: bool,
        b_crashed: bool,
        a_tm_stats: Option<TmRunStats>,
        b_tm_stats: Option<TmRunStats>,
    ) {
        let (a_outcome, b_outcome) = if self.use_adjusted {
            (result.a_adjusted_total, result.b_adjusted_total)
        } else {
            (result.a_total as f64, result.b_total as f64)
        };
        let outcome_order = compare_scores(a_outcome, b_outcome);
        let score_samples = u64::from(result.rounds);
        if result.a_idx == result.b_idx {
            let stats = &mut self.strategies[result.a_idx];
            stats.total += result.a_total + result.b_total;
            stats.adjusted_total += result.a_adjusted_total + result.b_adjusted_total;
            stats.score_samples += score_samples.saturating_mul(2);
            stats.matches += 2;
            match outcome_order {
                Ordering::Greater | Ordering::Less => {
                    stats.wins += 1;
                    stats.losses += 1;
                }
                Ordering::Equal => {
                    stats.draws += 2;
                }
            }
            if a_crashed || b_crashed {
                stats.crashed = true;
            }
            if let Some(pairwise) = self.pairwise.as_mut() {
                let pair = &mut pairwise[result.a_idx][result.b_idx];
                pair.a_total += result.a_total;
                pair.b_total += result.b_total;
                pair.a_adjusted_total += result.a_adjusted_total;
                pair.b_adjusted_total += result.b_adjusted_total;
                match outcome_order {
                    Ordering::Greater => pair.a_wins += 1,
                    Ordering::Less => pair.b_wins += 1,
                    Ordering::Equal => pair.draws += 1,
                }
            }
            if let Some(tm_stats) = a_tm_stats.as_ref() {
                let entry = stats.tm_stats.get_or_insert_with(TmRunStats::default);
                entry.merge(tm_stats);
            }
            if let Some(tm_stats) = b_tm_stats.as_ref() {
                let entry = stats.tm_stats.get_or_insert_with(TmRunStats::default);
                entry.merge(tm_stats);
            }
            return;
        }
        let (a_stats, b_stats) = if result.a_idx < result.b_idx {
            let (left, right) = self.strategies.split_at_mut(result.b_idx);
            let a_stats = &mut left[result.a_idx];
            let b_stats = &mut right[0];
            (a_stats, b_stats)
        } else {
            let (left, right) = self.strategies.split_at_mut(result.a_idx);
            let b_stats = &mut left[result.b_idx];
            let a_stats = &mut right[0];
            (a_stats, b_stats)
        };
        a_stats.total += result.a_total;
        b_stats.total += result.b_total;
        a_stats.adjusted_total += result.a_adjusted_total;
        b_stats.adjusted_total += result.b_adjusted_total;
        a_stats.score_samples += score_samples;
        b_stats.score_samples += score_samples;
        a_stats.matches += 1;
        b_stats.matches += 1;
        if a_crashed {
            a_stats.crashed = true;
        }
        if b_crashed {
            b_stats.crashed = true;
        }
        if let Some(tm_stats) = a_tm_stats.as_ref() {
            let entry = a_stats.tm_stats.get_or_insert_with(TmRunStats::default);
            entry.merge(tm_stats);
        }
        if let Some(tm_stats) = b_tm_stats.as_ref() {
            let entry = b_stats.tm_stats.get_or_insert_with(TmRunStats::default);
            entry.merge(tm_stats);
        }

        match outcome_order {
            Ordering::Greater => {
                a_stats.wins += 1;
                b_stats.losses += 1;
            }
            Ordering::Less => {
                b_stats.wins += 1;
                a_stats.losses += 1;
            }
            Ordering::Equal => {
                a_stats.draws += 1;
                b_stats.draws += 1;
            }
        }

        if let Some(pairwise) = self.pairwise.as_mut() {
            let pair = &mut pairwise[result.a_idx][result.b_idx];
            pair.a_total += result.a_total;
            pair.b_total += result.b_total;
            pair.a_adjusted_total += result.a_adjusted_total;
            pair.b_adjusted_total += result.b_adjusted_total;
            match outcome_order {
                Ordering::Greater => pair.a_wins += 1,
                Ordering::Less => pair.b_wins += 1,
                Ordering::Equal => pair.draws += 1,
            }

            if result.a_idx != result.b_idx {
                let reverse = &mut pairwise[result.b_idx][result.a_idx];
                reverse.a_total += result.b_total;
                reverse.b_total += result.a_total;
                reverse.a_adjusted_total += result.b_adjusted_total;
                reverse.b_adjusted_total += result.a_adjusted_total;
                match compare_scores(b_outcome, a_outcome) {
                    Ordering::Greater => reverse.a_wins += 1,
                    Ordering::Less => reverse.b_wins += 1,
                    Ordering::Equal => reverse.draws += 1,
                }
            }
        }
    }

    fn build_ranking(&self, specs: &[StrategySpec]) -> Vec<StrategyResult> {
        let mut ranking = Vec::new();
        for (idx, stats) in self.strategies.iter().enumerate() {
            let score_samples = stats.score_samples.max(1);
            let adjusted_avg = stats.adjusted_total / score_samples as f64;
            ranking.push(StrategyResult {
                id: specs[idx].id.clone(),
                name: specs[idx].name.clone(),
                total_payoff: stats.total,
                average_payoff: stats.total as f64 / score_samples as f64,
                adjusted_total_payoff: Some(stats.adjusted_total),
                adjusted_average_payoff: Some(adjusted_avg),
                matches: stats.matches,
                wins: stats.wins,
                losses: stats.losses,
                draws: stats.draws,
                crashed: stats.crashed,
                crash_count: stats.crash_count,
                tm_metrics: stats.tm_stats.as_ref().map(tm_metrics_from_stats),
            });
        }
        if self.use_adjusted {
            ranking.sort_by(|a, b| {
                let a_score = a.score(self.score_aggregation, true);
                let b_score = b.score(self.score_aggregation, true);
                b_score.partial_cmp(&a_score).unwrap_or(Ordering::Equal)
            });
        } else {
            ranking.sort_by(|a, b| {
                let a_score = a.score(self.score_aggregation, false);
                let b_score = b.score(self.score_aggregation, false);
                b_score.partial_cmp(&a_score).unwrap_or(Ordering::Equal)
            });
        }
        ranking
    }

    fn leaderboard(&self, specs: &[StrategySpec]) -> TournamentResults {
        TournamentResults {
            ranking: self.build_ranking(specs),
            pairwise: Vec::new(),
            dominance: Vec::new(),
        }
    }

    fn finalize(&self, specs: &[StrategySpec]) -> TournamentResults {
        let ranking = self.build_ranking(specs);

        let mut pairwise = Vec::new();
        if let Some(rows) = self.pairwise.as_ref() {
            for (i, row) in rows.iter().enumerate() {
                for (j, pair) in row.iter().enumerate() {
                    if i >= j {
                        continue;
                    }
                    if pair.a_total == 0
                        && pair.b_total == 0
                        && pair.a_wins == 0
                        && pair.b_wins == 0
                        && pair.draws == 0
                    {
                        continue;
                    }
                    pairwise.push(PairwiseResult {
                        a: specs[i].id.clone(),
                        b: specs[j].id.clone(),
                        a_total: pair.a_total,
                        b_total: pair.b_total,
                        a_adjusted_total: Some(pair.a_adjusted_total),
                        b_adjusted_total: Some(pair.b_adjusted_total),
                        a_wins: pair.a_wins,
                        b_wins: pair.b_wins,
                        draws: pair.draws,
                    });
                }
            }
        }

        let mut dominance = Vec::new();
        for pair in &pairwise {
            if pair.a_total > pair.b_total {
                dominance.push(DominanceEdge {
                    winner: pair.a.clone(),
                    loser: pair.b.clone(),
                });
            } else if pair.b_total > pair.a_total {
                dominance.push(DominanceEdge {
                    winner: pair.b.clone(),
                    loser: pair.a.clone(),
                });
            }
        }

        TournamentResults {
            ranking,
            pairwise,
            dominance,
        }
    }
}

fn matches_per_repetition(strategy_count: usize, self_play: bool) -> Option<usize> {
    if strategy_count == 0 {
        return Some(0);
    }
    if self_play {
        strategy_count.checked_mul(strategy_count)
    } else {
        strategy_count.checked_mul(strategy_count.saturating_sub(1))
    }
}

fn total_schedule_matches(
    strategy_count: usize,
    repetitions: u32,
    self_play: bool,
) -> Option<usize> {
    matches_per_repetition(strategy_count, self_play)?.checked_mul(repetitions as usize)
}

fn build_strategy_definitions(
    strategies: &[StrategySpec],
    _seed_deriver: &SeedDeriver,
) -> Vec<StrategyDefinition> {
    strategies
        .iter()
        .map(|spec| StrategyDefinition {
            id: spec.id.clone(),
            name: spec.name.clone(),
            kind: spec.kind.clone(),
            rng_seed_a: None,
            rng_seed_b: None,
        })
        .collect()
}

fn strategy_log_id(spec: &StrategySpec) -> String {
    match &spec.kind {
        StrategySpecKind::Fsm {
            index: Some(index), ..
        } => index.to_string(),
        StrategySpecKind::Ca { n, .. } => n.to_string(),
        StrategySpecKind::OneSidedTm {
            rule_code: Some(rule_code),
            ..
        } => rule_code.to_string(),
        _ => spec.id.clone(),
    }
}

fn build_strategy(spec: &StrategySpec, seed: u64) -> Box<dyn Strategy> {
    let _ = seed;
    match &spec.kind {
        StrategySpecKind::Fsm {
            start_state,
            outputs,
            input_mode,
            transitions,
            ..
        } => Box::new(FsmStrategy::new(
            spec.id.clone(),
            *start_state,
            outputs.clone(),
            input_mode.unwrap_or_default(),
            transitions.clone(),
        )),
        StrategySpecKind::Ca { n, k, r, t } => Box::new(CaStrategy::new(
            spec.id.clone(),
            *n,
            *k,
            (*r * 2.0).round() as u32,
            *t,
        )),
        StrategySpecKind::OneSidedTm {
            symbols,
            start_state,
            blank,
            fallback_symbol,
            max_steps_per_round,
            input_mode,
            output_map,
            transitions,
            ..
        } => Box::new(OneSidedTmStrategy::new(
            spec.id.clone(),
            *symbols,
            *start_state,
            *blank,
            fallback_symbol.unwrap_or(*blank),
            *max_steps_per_round,
            *input_mode,
            output_map.clone(),
            transitions.clone(),
        )),
    }
}
