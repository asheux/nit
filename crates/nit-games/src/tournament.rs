use crate::config::{
    BuiltinKind, NormalizedConfig, ParallelismConfig, ParallelismMode, StrategySpec,
    StrategySpecKind,
};
use crate::events::{EventWriter, GameEvent};
use crate::game::{Action, Outcome};
use crate::fast_eval::{evaluate_match, CycleMetadata, FastStrategyModel};
use crate::history::History;
use crate::history_log::{HistoryWriter, MatchHistory};
use crate::output::{
    DominanceEdge, PairwiseResult, RunSummary, StrategyDefinition, StrategyResult,
    TournamentResults,
};
use crate::strategy::{
    AlwaysCooperate, AlwaysDefect, FsmStrategy, GrimTrigger, MemoryStrategy, OneSidedTmStrategy,
    RandomStrategy, Strategy, TitForTat, TmRunStats, WinStayLoseShift,
};
use nit_utils::hashing::{stable_hash_bytes, XorShift64};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
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
    pub last_action_a: Option<Action>,
    pub last_action_b: Option<Action>,
    pub last_payoff_a: Option<i32>,
    pub last_payoff_b: Option<i32>,
    pub last_outcome: Option<Outcome>,
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
}

#[derive(Clone, Debug)]
pub struct MatchResult {
    pub a_idx: usize,
    pub b_idx: usize,
    pub a_total: i64,
    pub b_total: i64,
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
    schedule: Vec<Matchup>,
    match_index: usize,
    current: Option<MatchSession>,
    results: TournamentAccumulator,
    strategies: Vec<StrategySpec>,
    definitions: Vec<StrategyDefinition>,
    seed_deriver: SeedDeriver,
    event_writer: Option<EventWriter>,
    history_writer: Option<HistoryWriter>,
    last_round: Option<RoundSnapshot>,
}

#[derive(Clone, Debug)]
struct Matchup {
    match_id: usize,
    a_idx: usize,
    b_idx: usize,
    repetition: u32,
}

struct MatchSession {
    matchup: Matchup,
    history: History,
    a_strategy: Box<dyn Strategy>,
    b_strategy: Box<dyn Strategy>,
    noise_rng: XorShift64,
    history_actions_a: String,
    history_actions_b: String,
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
    a_wins: u32,
    b_wins: u32,
    draws: u32,
}

struct TournamentAccumulator {
    strategies: Vec<StrategyStats>,
    pairwise: Vec<Vec<PairStats>>,
}

impl TournamentRunner {
    pub fn new(mut config: NormalizedConfig) -> Self {
        let seed = config.seed.unwrap_or(0);
        config.seed = Some(seed);
        let schedule = build_schedule(
            config.strategies.len(),
            config.repetitions,
            config.self_play,
        );
        let seed_deriver = SeedDeriver::new(seed);
        let definitions = build_strategy_definitions(&config.strategies, &seed_deriver);
        let results = TournamentAccumulator::new(config.strategies.len());
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
            event_writer: None,
            history_writer: None,
            last_round: None,
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

    pub fn is_done(&self) -> bool {
        self.match_index >= self.schedule.len() && self.current.is_none()
    }

    pub fn progress(&self) -> Option<TournamentProgress> {
        let current = self.current.as_ref()?;
        let matchup = &current.matchup;
        let a = self.strategies.get(matchup.a_idx)?.id.clone();
        let b = self.strategies.get(matchup.b_idx)?.id.clone();
        Some(TournamentProgress {
            match_index: self.match_index.saturating_add(1),
            total_matches: self.schedule.len().max(1),
            round: current.round,
            rounds: current.rounds_total,
            a,
            b,
            last_action_a: self.last_round.as_ref().map(|r| r.a_action),
            last_action_b: self.last_round.as_ref().map(|r| r.b_action),
            last_payoff_a: self.last_round.as_ref().map(|r| r.a_payoff),
            last_payoff_b: self.last_round.as_ref().map(|r| r.b_payoff),
            last_outcome: self
                .last_round
                .as_ref()
                .map(|r| Outcome::from_actions(r.a_action, r.b_action)),
        })
    }

    pub fn match_snapshot(&self) -> Option<MatchSnapshot> {
        let current = self.current.as_ref()?;
        let matchup = &current.matchup;
        let a = self.strategies.get(matchup.a_idx)?.id.clone();
        let b = self.strategies.get(matchup.b_idx)?.id.clone();
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
        for _ in 0..steps {
            if self.is_done() {
                break;
            }
            if self.current.is_none() {
                if let Some(matchup) = self.schedule.get(self.match_index).cloned() {
                    let session = MatchSession::new(
                        matchup,
                        &self.config,
                        &self.strategies,
                        &self.seed_deriver,
                        self.history_writer.is_some(),
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
                    self.current = Some(session);
                } else {
                    break;
                }
            }

            if let Some(mut session) = self.current.take() {
                let snapshot = self.play_round(&mut session);
                self.last_round = Some(snapshot.clone());
                if session.round >= session.rounds_total {
                    let result = MatchResult {
                        a_idx: session.matchup.a_idx,
                        b_idx: session.matchup.b_idx,
                        a_total: session.a_total,
                        b_total: session.b_total,
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
                    self.results
                        .apply_match(
                            result,
                            session.a_crashed,
                            session.b_crashed,
                            session.a_strategy.tm_stats().cloned(),
                            session.b_strategy.tm_stats().cloned(),
                        );
                    self.match_index += 1;
                    if self.is_done() {
                        self.emit(GameEvent::TournamentEnd {
                            timestamp: EventWriter::timestamp(),
                        });
                    }
                } else {
                    self.current = Some(session);
                }
            }
        }
    }

    pub fn results(&self) -> TournamentResults {
        self.results.finalize(&self.strategies)
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
        let a = self.strategies[session.matchup.a_idx].id.clone();
        let b = self.strategies[session.matchup.b_idx].id.clone();
        let a_moves = session.history_actions_a.clone();
        let b_moves = session.history_actions_b.clone();
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
            a_incoming: b_moves,
            b_incoming: a_moves,
            score_idx: session.history_scores.clone(),
            a_score: session.a_total,
            b_score: session.b_total,
            a_initial: session.history_actions_a.chars().next(),
            b_initial: session.history_actions_b.chars().next(),
            cycle: None,
        };
        let _ = writer.write(&record);
    }

    fn play_round(&mut self, session: &mut MatchSession) -> RoundSnapshot {
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
            a_payoff: outcome.snapshot.a_payoff,
            b_payoff: outcome.snapshot.b_payoff,
        });
        outcome.snapshot
    }

}

fn play_round_core(session: &mut MatchSession, config: &NormalizedConfig) -> RoundOutcome {
    let (a_action, b_action, a_crash_now, b_crash_now) = {
        let mut a_crash = false;
        let mut b_crash = false;
        let a_action = if session.a_crashed {
            Action::Defect
        } else {
            match catch_unwind(AssertUnwindSafe(|| {
                session.a_strategy.next_action(&session.history, true)
            })) {
                Ok(action) => action,
                Err(_) => {
                    a_crash = true;
                    Action::Defect
                }
            }
        };
        let b_action = if session.b_crashed {
            Action::Defect
        } else {
            match catch_unwind(AssertUnwindSafe(|| {
                session.b_strategy.next_action(&session.history, false)
            })) {
                Ok(action) => action,
                Err(_) => {
                    b_crash = true;
                    Action::Defect
                }
            }
        };
        (a_action, b_action, a_crash, b_crash)
    };

    if a_crash_now {
        session.a_crashed = true;
    }
    if b_crash_now {
        session.b_crashed = true;
    }

    let a_action = apply_noise(config.noise, a_action, &mut session.noise_rng);
    let b_action = apply_noise(config.noise, b_action, &mut session.noise_rng);
    let (a_payoff, b_payoff) = config.payoff.payoffs(a_action, b_action);
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
    }
    session.round += 1;

    RoundOutcome {
        snapshot: RoundSnapshot {
            a_action,
            b_action,
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
    schedule: Vec<Matchup>,
    definitions: Vec<StrategyDefinition>,
    seed_deriver: SeedDeriver,
    fast_models: Vec<Option<FastStrategyModel>>,
}

impl TournamentKernel {
    pub fn new(mut config: NormalizedConfig) -> Self {
        let seed = config.seed.unwrap_or(0);
        config.seed = Some(seed);
        let schedule = build_schedule(
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
    ) -> TournamentResults {
        let total_matches = self.schedule.len();
        let mut results = TournamentAccumulator::new(self.config.strategies.len());
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

        for matchup in &self.schedule {
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
                matchup,
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

        if let Some(writer) = event_writer.as_mut() {
            let _ = writer.write(&GameEvent::TournamentEnd {
                timestamp: EventWriter::timestamp(),
            });
        }
        results.finalize(&self.config.strategies)
    }

    fn run_parallel(
        &self,
        parallelism: Parallelism,
        event_sender: Option<Sender<GameEvent>>,
        include_rounds: bool,
        history_sender: Option<Sender<MatchHistory>>,
    ) -> TournamentResults {
        let total_matches = self.schedule.len();
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

        let run = || {
            self.schedule
                .par_iter()
                .map(move |matchup| {
                    let event_tx = event_sender_for_run.as_ref().map(Sender::clone);
                    let history_tx = history_sender_for_run.as_ref().map(Sender::clone);
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
                        matchup,
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

        let mut results = TournamentAccumulator::new(self.config.strategies.len());
        for outcome in outcomes {
            results.apply_match(
                outcome.result,
                outcome.a_crashed,
                outcome.b_crashed,
                outcome.a_tm_stats,
                outcome.b_tm_stats,
            );
        }
        results.finalize(&self.config.strategies)
    }
}

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
    let a_id = strategies[matchup.a_idx].id.as_str();
    let b_id = strategies[matchup.b_idx].id.as_str();
    let match_index = matchup.match_id + 1;
    let owned_ids = if log_events || log_history {
        Some((a_id.to_string(), b_id.to_string()))
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
        if let Some((a_model, b_model)) = fast_models
            .and_then(|models| {
                let a = models.get(matchup.a_idx).and_then(|m| m.as_ref());
                let b = models.get(matchup.b_idx).and_then(|m| m.as_ref());
                a.zip(b)
            })
        {
            let eval = evaluate_match(
                a_model,
                b_model,
                config.rounds,
                config.payoff,
                false,
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
            return MatchOutcome {
                result: MatchResult {
                    a_idx: matchup.a_idx,
                    b_idx: matchup.b_idx,
                    a_total: eval.a_total,
                    b_total: eval.b_total,
                    repetition: matchup.repetition,
                    match_id: matchup.match_id,
                },
                a_crashed: false,
                b_crashed: false,
                a_tm_stats: None,
                b_tm_stats: None,
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
    for _ in 0..session.rounds_total {
        let outcome = play_round_core(&mut session, config);
        if outcome.a_crash_now && log_events {
            emit_event(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: a_id.to_string(),
                error: "panic in strategy".into(),
            });
        }
        if outcome.b_crash_now && log_events {
            emit_event(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: b_id.to_string(),
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
            cycle_meta = evaluate_match(
                a_model,
                b_model,
                config.rounds,
                config.payoff,
                true,
            )
            .cycle;
        }
    }

    if log_history {
        let a_moves = session.history_actions_a.clone();
        let b_moves = session.history_actions_b.clone();
        let (a_owned, b_owned) = owned_ids.as_ref().expect("owned ids");
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
            a_incoming: b_moves,
            b_incoming: a_moves,
            score_idx: session.history_scores.clone(),
            a_score: session.a_total,
            b_score: session.b_total,
            a_initial: session.history_actions_a.chars().next(),
            b_initial: session.history_actions_b.chars().next(),
            cycle: cycle_meta,
        });
    }

    MatchOutcome {
        result: MatchResult {
            a_idx: matchup.a_idx,
            b_idx: matchup.b_idx,
            a_total: session.a_total,
            b_total: session.b_total,
            repetition: matchup.repetition,
            match_id: matchup.match_id,
        },
        a_crashed: session.a_crashed,
        b_crashed: session.b_crashed,
        a_tm_stats: session.a_strategy.tm_stats().cloned(),
        b_tm_stats: session.b_strategy.tm_stats().cloned(),
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
    fn new(n: usize) -> Self {
        Self {
            strategies: vec![
                StrategyStats {
                    total: 0,
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
            pairwise: vec![vec![PairStats::default(); n]; n],
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
        if result.a_idx == result.b_idx {
            let stats = &mut self.strategies[result.a_idx];
            stats.total += result.a_total + result.b_total;
            stats.matches += 1;
            stats.draws += 1;
            if a_crashed || b_crashed {
                stats.crashed = true;
            }
            let pair = &mut self.pairwise[result.a_idx][result.b_idx];
            pair.a_total += result.a_total;
            pair.b_total += result.b_total;
            pair.draws += 1;
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

        if result.a_total > result.b_total {
            a_stats.wins += 1;
            b_stats.losses += 1;
        } else if result.b_total > result.a_total {
            b_stats.wins += 1;
            a_stats.losses += 1;
        } else {
            a_stats.draws += 1;
            b_stats.draws += 1;
        }

        let pair = &mut self.pairwise[result.a_idx][result.b_idx];
        pair.a_total += result.a_total;
        pair.b_total += result.b_total;
        if result.a_total > result.b_total {
            pair.a_wins += 1;
        } else if result.b_total > result.a_total {
            pair.b_wins += 1;
        } else {
            pair.draws += 1;
        }

        if result.a_idx != result.b_idx {
            let reverse = &mut self.pairwise[result.b_idx][result.a_idx];
            reverse.a_total += result.b_total;
            reverse.b_total += result.a_total;
            if result.b_total > result.a_total {
                reverse.a_wins += 1;
            } else if result.a_total > result.b_total {
                reverse.b_wins += 1;
            } else {
                reverse.draws += 1;
            }
        }
    }

    fn finalize(&self, specs: &[StrategySpec]) -> TournamentResults {
        let mut ranking = Vec::new();
        for (idx, stats) in self.strategies.iter().enumerate() {
            let matches = stats.matches.max(1);
            ranking.push(StrategyResult {
                id: specs[idx].id.clone(),
                name: specs[idx].name.clone(),
                total_payoff: stats.total,
                average_payoff: stats.total as f64 / matches as f64,
                matches: stats.matches,
                wins: stats.wins,
                losses: stats.losses,
                draws: stats.draws,
                crashed: stats.crashed,
                crash_count: stats.crash_count,
                tm_metrics: stats.tm_stats.as_ref().map(|tm| {
                    let rounds = tm.rounds.max(1);
                    let avg_steps = tm.steps as f64 / rounds as f64;
                    let output_rate = tm.output_events as f64 / rounds as f64;
                    let fallback_rate = tm.fallback as f64 / rounds as f64;
                    crate::output::TmDerivedMetrics {
                        rounds: tm.rounds,
                        avg_steps_per_move: avg_steps,
                        max_steps_hit_count: tm.max_steps_hits,
                        output_event_hit_rate: output_rate,
                        fallback_rate,
                    }
                }),
            });
        }
        ranking.sort_by(|a, b| b.total_payoff.cmp(&a.total_payoff));

        let mut pairwise = Vec::new();
        for (i, row) in self.pairwise.iter().enumerate() {
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
                    a_wins: pair.a_wins,
                    b_wins: pair.b_wins,
                    draws: pair.draws,
                });
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

fn build_schedule(count: usize, repetitions: u32, self_play: bool) -> Vec<Matchup> {
    let mut schedule = Vec::new();
    if count == 0 || repetitions == 0 {
        return schedule;
    }
    let mut match_id = 0usize;
    for rep in 0..repetitions {
        for i in 0..count {
            if self_play {
                schedule.push(Matchup {
                    match_id,
                    a_idx: i,
                    b_idx: i,
                    repetition: rep,
                });
                match_id += 1;
            }
            for j in (i + 1)..count {
                schedule.push(Matchup {
                    match_id,
                    a_idx: i,
                    b_idx: j,
                    repetition: rep,
                });
                match_id += 1;
            }
        }
    }
    schedule
}

fn build_strategy_definitions(
    strategies: &[StrategySpec],
    seed_deriver: &SeedDeriver,
) -> Vec<StrategyDefinition> {
    strategies
        .iter()
        .map(|spec| StrategyDefinition {
            id: spec.id.clone(),
            name: spec.name.clone(),
            kind: spec.kind.clone(),
            rng_seed_a: matches!(spec.kind, StrategySpecKind::Random { .. })
                .then(|| seed_deriver.base_strategy_seed(MatchRole::A, &spec.id)),
            rng_seed_b: matches!(spec.kind, StrategySpecKind::Random { .. })
                .then(|| seed_deriver.base_strategy_seed(MatchRole::B, &spec.id)),
        })
        .collect()
}

fn build_strategy(spec: &StrategySpec, seed: u64) -> Box<dyn Strategy> {
    match &spec.kind {
        StrategySpecKind::Builtin { builtin } => match builtin {
            BuiltinKind::AllC => Box::new(AlwaysCooperate::new(spec.id.clone())),
            BuiltinKind::AllD => Box::new(AlwaysDefect::new(spec.id.clone())),
            BuiltinKind::TitForTat => Box::new(TitForTat::new(spec.id.clone())),
            BuiltinKind::GrimTrigger => Box::new(GrimTrigger::new(spec.id.clone())),
            BuiltinKind::WinStayLoseShift => Box::new(WinStayLoseShift::new(spec.id.clone())),
        },
        StrategySpecKind::Random { p_cooperate } => {
            Box::new(RandomStrategy::new(spec.id.clone(), seed, *p_cooperate))
        }
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
        StrategySpecKind::Memory { n, initial, table } => Box::new(MemoryStrategy::new(
            spec.id.clone(),
            *n,
            *initial,
            table.clone(),
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
