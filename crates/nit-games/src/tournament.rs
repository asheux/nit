use crate::config::{BuiltinKind, NormalizedConfig, StrategySpec, StrategySpecKind};
use crate::events::{EventWriter, GameEvent};
use crate::game::{Action, Outcome};
use crate::history::History;
use crate::history_log::{HistoryWriter, MatchHistory};
use crate::output::{
    DominanceEdge, PairwiseResult, RunSummary, StrategyDefinition, StrategyResult,
    TournamentResults,
};
use crate::strategy::{
    AlwaysCooperate, AlwaysDefect, FsmStrategy, GrimTrigger, MemoryStrategy, RandomStrategy,
    Strategy, TitForTat, WinStayLoseShift,
};
use nit_utils::hashing::{stable_hash_bytes, XorShift64};
use std::panic::{catch_unwind, AssertUnwindSafe};

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
pub struct MatchResult {
    pub a_idx: usize,
    pub b_idx: usize,
    pub a_total: i64,
    pub b_total: i64,
    pub repetition: u32,
}

pub struct TournamentRunner {
    config: NormalizedConfig,
    seed: u64,
    schedule: Vec<Matchup>,
    match_index: usize,
    current: Option<MatchSession>,
    results: TournamentAccumulator,
    strategies: Vec<StrategySpec>,
    pool: Vec<StrategyPair>,
    definitions: Vec<StrategyDefinition>,
    noise_rng: XorShift64,
    event_writer: Option<EventWriter>,
    history_writer: Option<HistoryWriter>,
    last_round: Option<RoundSnapshot>,
}

#[derive(Clone, Debug)]
struct Matchup {
    a_idx: usize,
    b_idx: usize,
    repetition: u32,
}

struct MatchSession {
    matchup: Matchup,
    history: History,
    history_actions_a: String,
    history_actions_b: String,
    history_scores: String,
    round: u32,
    rounds_total: u32,
    a_total: i64,
    b_total: i64,
    a_crashed: bool,
    b_crashed: bool,
}

#[derive(Clone, Debug)]
struct RoundSnapshot {
    a_action: Action,
    b_action: Action,
    a_payoff: i32,
    b_payoff: i32,
}

struct StrategyPair {
    a: Box<dyn Strategy>,
    b: Box<dyn Strategy>,
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
        let pool = build_strategy_pool(&config.strategies, seed);
        let definitions = build_strategy_definitions(&config.strategies, seed);
        let noise_rng = XorShift64::new(seed ^ 0x9e3779b97f4a7c15);
        let results = TournamentAccumulator::new(config.strategies.len());
        Self {
            config: config.clone(),
            seed,
            schedule,
            match_index: 0,
            current: None,
            results,
            strategies: config.strategies.clone(),
            pool,
            definitions,
            noise_rng,
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
                        self.config.rounds,
                        self.config.max_memory_n,
                        self.history_writer.is_some(),
                    );
                    self.reset_match_strategies(session.matchup.a_idx, session.matchup.b_idx);
                    self.emit(GameEvent::MatchStart {
                        timestamp: EventWriter::timestamp(),
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
                    };
                    self.emit(GameEvent::MatchEnd {
                        timestamp: EventWriter::timestamp(),
                        match_index: self.match_index + 1,
                        a_total: session.a_total,
                        b_total: session.b_total,
                    });
                    self.emit_history(&session);
                    self.results
                        .apply_match(result, session.a_crashed, session.b_crashed);
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

    pub fn finish(mut self, timestamp: String) -> RunSummary {
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
            schema_version: self.config.schema_version,
            timestamp,
            seed: self.seed,
            config: self.config.clone(),
            strategies: self.definitions.clone(),
            results,
            event_log,
            history_log,
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
        };
        let _ = writer.write(&record);
    }

    fn play_round(&mut self, session: &mut MatchSession) -> RoundSnapshot {
        let a_idx = session.matchup.a_idx;
        let b_idx = session.matchup.b_idx;
        let a_id = self.strategies[a_idx].id.clone();
        let b_id = self.strategies[b_idx].id.clone();

        let (a_action, b_action, a_crash_now, b_crash_now) = {
            let (a_strategy, b_strategy) = self.borrow_match_strategies(a_idx, b_idx);
            let mut a_crash = false;
            let mut b_crash = false;
            let a_action = if session.a_crashed {
                Action::Defect
            } else {
                match catch_unwind(AssertUnwindSafe(|| {
                    a_strategy.next_action(&session.history, true)
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
                    b_strategy.next_action(&session.history, false)
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
            self.results.strategies[a_idx].crash_count += 1;
            self.results.strategies[a_idx].crashed = true;
            self.emit(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: a_id,
                error: "panic in strategy".into(),
            });
        }
        if b_crash_now {
            session.b_crashed = true;
            self.results.strategies[b_idx].crash_count += 1;
            self.results.strategies[b_idx].crashed = true;
            self.emit(GameEvent::StrategyError {
                timestamp: EventWriter::timestamp(),
                strategy_id: b_id,
                error: "panic in strategy".into(),
            });
        }

        let a_action = self.apply_noise(a_action);
        let b_action = self.apply_noise(b_action);
        let (a_payoff, b_payoff) = self.config.payoff.payoffs(a_action, b_action);
        let outcome = Outcome::from_actions(a_action, b_action);
        session.a_total += a_payoff as i64;
        session.b_total += b_payoff as i64;
        session.history.push(a_action, b_action);
        if self.history_writer.is_some() {
            session.history_actions_a.push(a_action.as_char());
            session.history_actions_b.push(b_action.as_char());
            session.history_scores.push(outcome_char(outcome));
        }
        session.round += 1;

        let snapshot = RoundSnapshot {
            a_action,
            b_action,
            a_payoff,
            b_payoff,
        };
        self.emit(GameEvent::Round {
            timestamp: EventWriter::timestamp(),
            match_index: self.match_index + 1,
            round: session.round,
            a_action: a_action.as_char(),
            b_action: b_action.as_char(),
            a_payoff,
            b_payoff,
        });
        snapshot
    }

    fn apply_noise(&mut self, action: Action) -> Action {
        if self.config.noise <= 0.0 {
            return action;
        }
        if self.noise_rng.next_f32() < self.config.noise {
            action.flip()
        } else {
            action
        }
    }

    fn reset_match_strategies(&mut self, a_idx: usize, b_idx: usize) {
        let (a_strategy, b_strategy) = self.borrow_match_strategies(a_idx, b_idx);
        a_strategy.reset();
        b_strategy.reset();
    }

    fn borrow_match_strategies(
        &mut self,
        a_idx: usize,
        b_idx: usize,
    ) -> (&mut dyn Strategy, &mut dyn Strategy) {
        if a_idx == b_idx {
            let pair = &mut self.pool[a_idx];
            return (&mut *pair.a, &mut *pair.b);
        }
        let (left, right) = if a_idx < b_idx {
            let (left, right) = self.pool.split_at_mut(b_idx);
            (left, right)
        } else {
            let (left, right) = self.pool.split_at_mut(a_idx);
            (left, right)
        };
        if a_idx < b_idx {
            let a = &mut *left[a_idx].a;
            let b = &mut *right[0].b;
            (a, b)
        } else {
            let a = &mut *right[0].a;
            let b = &mut *left[b_idx].b;
            (a, b)
        }
    }
}

impl MatchSession {
    fn new(matchup: Matchup, rounds_total: u32, max_memory: usize, record_history: bool) -> Self {
        Self {
            matchup,
            history: History::new(max_memory),
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
            history_scores: if record_history {
                String::with_capacity(rounds_total as usize)
            } else {
                String::new()
            },
            round: 0,
            rounds_total,
            a_total: 0,
            b_total: 0,
            a_crashed: false,
            b_crashed: false,
        }
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
                };
                n
            ],
            pairwise: vec![vec![PairStats::default(); n]; n],
        }
    }

    fn apply_match(&mut self, result: MatchResult, a_crashed: bool, b_crashed: bool) {
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
    for rep in 0..repetitions {
        for i in 0..count {
            if self_play {
                schedule.push(Matchup {
                    a_idx: i,
                    b_idx: i,
                    repetition: rep,
                });
            }
            for j in (i + 1)..count {
                schedule.push(Matchup {
                    a_idx: i,
                    b_idx: j,
                    repetition: rep,
                });
            }
        }
    }
    schedule
}

fn build_strategy_pool(strategies: &[StrategySpec], seed: u64) -> Vec<StrategyPair> {
    strategies
        .iter()
        .map(|spec| {
            let seed_a = derive_seed(seed, &spec.id, "A");
            let seed_b = derive_seed(seed, &spec.id, "B");
            StrategyPair {
                a: build_strategy(spec, seed_a),
                b: build_strategy(spec, seed_b),
            }
        })
        .collect()
}

fn build_strategy_definitions(strategies: &[StrategySpec], seed: u64) -> Vec<StrategyDefinition> {
    strategies
        .iter()
        .map(|spec| StrategyDefinition {
            id: spec.id.clone(),
            name: spec.name.clone(),
            kind: spec.kind.clone(),
            rng_seed_a: matches!(spec.kind, StrategySpecKind::Random { .. })
                .then(|| derive_seed(seed, &spec.id, "A")),
            rng_seed_b: matches!(spec.kind, StrategySpecKind::Random { .. })
                .then(|| derive_seed(seed, &spec.id, "B")),
        })
        .collect()
}

fn derive_seed(seed: u64, id: &str, role: &str) -> u64 {
    stable_hash_bytes(format!("{seed}:{id}:{role}").as_bytes())
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
            output,
            transitions,
            ..
        } => Box::new(FsmStrategy::new(
            spec.id.clone(),
            *start_state,
            output.clone(),
            transitions.clone(),
        )),
        StrategySpecKind::Memory { n, initial, table } => Box::new(MemoryStrategy::new(
            spec.id.clone(),
            *n,
            *initial,
            table.clone(),
        )),
    }
}
