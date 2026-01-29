use crate::game::{Action, Outcome};
use crate::history::History;
use nit_utils::hashing::XorShift64;
use serde::{Deserialize, Serialize};

pub trait Strategy: Send {
    fn id(&self) -> &str;
    fn reset(&mut self);
    fn next_action(&mut self, history: &History, player_a: bool) -> Action;
    fn tm_stats(&self) -> Option<&TmRunStats> {
        None
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StrategyKind {
    Builtin,
    Random,
    Fsm,
    Memory,
    OneSidedTm,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    OpponentLastAction,
    SelfLastAction,
    JointLastAction,
}

impl Default for InputMode {
    fn default() -> Self {
        InputMode::OpponentLastAction
    }
}

impl InputMode {
    pub fn alphabet_size(self) -> usize {
        match self {
            InputMode::OpponentLastAction | InputMode::SelfLastAction => 2,
            InputMode::JointLastAction => 4,
        }
    }

    pub fn symbol_from_actions(self, self_action: Action, opp_action: Action) -> usize {
        match self {
            InputMode::OpponentLastAction => match opp_action {
                Action::Cooperate => 0,
                Action::Defect => 1,
            },
            InputMode::SelfLastAction => match self_action {
                Action::Cooperate => 0,
                Action::Defect => 1,
            },
            InputMode::JointLastAction => Outcome::from_actions(self_action, opp_action).index(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TmMove {
    #[serde(rename = "L")]
    Left,
    #[serde(rename = "R")]
    Right,
    #[serde(rename = "S")]
    Stay,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct TmTransition {
    pub write: u8,
    #[serde(rename = "move")]
    pub move_dir: TmMove,
    pub next: u16,
}

pub fn decode_tm_rule_code_wolfram(
    rule_code: u64,
    states: usize,
    symbols: usize,
) -> (Vec<TmTransition>, u64) {
    let total = states.saturating_mul(symbols);
    let mut transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Left,
            next: 1,
        };
        total
    ];
    if states == 0 || symbols == 0 {
        return (transitions, rule_code);
    }
    let base = (symbols as u64) * (states as u64) * 2;
    if base == 0 {
        return (transitions, rule_code);
    }
    let mut code = rule_code;
    for state in (1..=states).rev() {
        for read in 0..symbols {
            let digit = code % base;
            code /= base;
            let move_idx = (digit % 2) as u8;
            let write = ((digit / 2) % symbols as u64) as u8;
            let next = (digit / (2 * symbols as u64)) as u16 + 1;
            let move_dir = if move_idx == 0 {
                TmMove::Left
            } else {
                TmMove::Right
            };
            let idx = (state - 1) * symbols + read;
            if let Some(slot) = transitions.get_mut(idx) {
                *slot = TmTransition {
                    write,
                    move_dir,
                    next,
                };
            }
        }
    }
    (transitions, code)
}

#[derive(Clone, Debug)]
pub struct AlwaysCooperate {
    id: String,
}

impl AlwaysCooperate {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

impl Strategy for AlwaysCooperate {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {}

    fn next_action(&mut self, _history: &History, _player_a: bool) -> Action {
        Action::Cooperate
    }
}

#[derive(Clone, Debug)]
pub struct AlwaysDefect {
    id: String,
}

impl AlwaysDefect {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

impl Strategy for AlwaysDefect {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {}

    fn next_action(&mut self, _history: &History, _player_a: bool) -> Action {
        Action::Defect
    }
}

#[derive(Clone, Debug)]
pub struct TitForTat {
    id: String,
}

impl TitForTat {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

impl Strategy for TitForTat {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {}

    fn next_action(&mut self, history: &History, player_a: bool) -> Action {
        history
            .last_actions_for(player_a)
            .map(|(_, opp)| opp)
            .unwrap_or(Action::Cooperate)
    }
}

#[derive(Clone, Debug)]
pub struct GrimTrigger {
    id: String,
    triggered: bool,
}

impl GrimTrigger {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            triggered: false,
        }
    }
}

impl Strategy for GrimTrigger {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.triggered = false;
    }

    fn next_action(&mut self, history: &History, player_a: bool) -> Action {
        if !self.triggered {
            if let Some((_, opp)) = history.last_actions_for(player_a) {
                if matches!(opp, Action::Defect) {
                    self.triggered = true;
                }
            }
        }
        if self.triggered {
            Action::Defect
        } else {
            Action::Cooperate
        }
    }
}

#[derive(Clone, Debug)]
pub struct WinStayLoseShift {
    id: String,
}

impl WinStayLoseShift {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

impl Strategy for WinStayLoseShift {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {}

    fn next_action(&mut self, history: &History, player_a: bool) -> Action {
        let Some((self_action, opp_action)) = history.last_actions_for(player_a) else {
            return Action::Cooperate;
        };
        let last_self = self_action;
        let last = Outcome::from_actions(self_action, opp_action);
        match last {
            Outcome::CC | Outcome::DD => last_self,
            Outcome::CD | Outcome::DC => last_self.flip(),
        }
    }
}

#[derive(Clone)]
pub struct RandomStrategy {
    id: String,
    rng: XorShift64,
    p_cooperate: f32,
}

impl RandomStrategy {
    pub fn new(id: impl Into<String>, seed: u64, p_cooperate: f32) -> Self {
        Self {
            id: id.into(),
            rng: XorShift64::new(seed),
            p_cooperate: p_cooperate.clamp(0.0, 1.0),
        }
    }
}

impl Strategy for RandomStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {}

    fn next_action(&mut self, _history: &History, _player_a: bool) -> Action {
        if self.rng.next_f32() < self.p_cooperate {
            Action::Cooperate
        } else {
            Action::Defect
        }
    }
}

#[derive(Clone, Debug)]
pub struct FsmStrategy {
    id: String,
    start_state: usize,
    state: usize,
    outputs: Vec<Action>,
    input_mode: InputMode,
    alphabet: usize,
    transitions: Vec<usize>,
}

impl FsmStrategy {
    pub fn new(
        id: impl Into<String>,
        start_state: usize,
        outputs: Vec<Action>,
        input_mode: InputMode,
        transitions: Vec<Vec<usize>>,
    ) -> Self {
        let alphabet = input_mode.alphabet_size();
        let mut flat = Vec::new();
        for row in transitions {
            for entry in row {
                flat.push(entry);
            }
        }
        Self {
            id: id.into(),
            start_state,
            state: start_state,
            outputs,
            input_mode,
            alphabet,
            transitions: flat,
        }
    }
}

impl Strategy for FsmStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.state = self.start_state;
    }

    fn next_action(&mut self, history: &History, player_a: bool) -> Action {
        if let Some((self_action, opp_action)) = history.last_actions_for(player_a) {
            let symbol = self.input_mode.symbol_from_actions(self_action, opp_action);
            let idx = self.state.saturating_mul(self.alphabet) + symbol;
            if let Some(next) = self.transitions.get(idx) {
                self.state = *next;
            }
        }
        self.outputs
            .get(self.state)
            .copied()
            .unwrap_or(Action::Cooperate)
    }
}

#[derive(Clone, Debug)]
pub struct MemoryStrategy {
    id: String,
    n: usize,
    initial: Action,
    table: Vec<Action>,
}

impl MemoryStrategy {
    pub fn new(id: impl Into<String>, n: usize, initial: Action, table: Vec<Action>) -> Self {
        Self {
            id: id.into(),
            n,
            initial,
            table,
        }
    }
}

impl Strategy for MemoryStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {}

    fn next_action(&mut self, history: &History, player_a: bool) -> Action {
        let Some(idx) = history.memory_index(player_a, self.n) else {
            return self.initial;
        };
        self.table.get(idx).copied().unwrap_or(self.initial)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TmRunStats {
    pub rounds: u64,
    pub steps: u64,
    pub output_events: u64,
    pub fallback: u64,
    pub max_steps_hits: u64,
}

impl TmRunStats {
    pub fn merge(&mut self, other: &TmRunStats) {
        self.rounds = self.rounds.saturating_add(other.rounds);
        self.steps = self.steps.saturating_add(other.steps);
        self.output_events = self.output_events.saturating_add(other.output_events);
        self.fallback = self.fallback.saturating_add(other.fallback);
        self.max_steps_hits = self.max_steps_hits.saturating_add(other.max_steps_hits);
    }
}

#[derive(Clone, Debug)]
pub struct OneSidedTmStrategy {
    id: String,
    symbols: u8,
    start_state: u16,
    blank: u8,
    fallback_symbol: u8,
    max_steps_per_round: u32,
    input_mode: InputMode,
    output_map: Vec<Action>,
    transitions: Vec<TmTransition>,
    tape: Vec<u8>,
    head: usize,
    last_history_len: usize,
    stats: TmRunStats,
}

impl OneSidedTmStrategy {
    pub fn new(
        id: impl Into<String>,
        symbols: u8,
        start_state: u16,
        blank: u8,
        fallback_symbol: u8,
        max_steps_per_round: u32,
        input_mode: InputMode,
        output_map: Vec<Action>,
        transitions: Vec<TmTransition>,
    ) -> Self {
        Self {
            id: id.into(),
            symbols,
            start_state,
            blank,
            fallback_symbol,
            max_steps_per_round,
            input_mode,
            output_map,
            transitions,
            tape: vec![blank],
            head: 0,
            last_history_len: 0,
            stats: TmRunStats::default(),
        }
    }

    pub fn stats(&self) -> &TmRunStats {
        &self.stats
    }

    fn append_history(&mut self, history: &History, player_a: bool) {
        let history_len = history.len();
        if history_len == 0 || history_len == self.last_history_len {
            return;
        }
        if let Some((self_action, opp_action)) = history.last_actions_for(player_a) {
            let symbol = self.input_mode.symbol_from_actions(self_action, opp_action) as u8;
            self.tape.push(symbol);
            self.last_history_len = history_len;
        }
    }

    fn output_from_symbol(&self, symbol: u8) -> Action {
        self.output_map
            .get(symbol as usize)
            .copied()
            .unwrap_or(Action::Cooperate)
    }

    fn fallback_action(&self) -> Action {
        self.output_map
            .get(self.fallback_symbol as usize)
            .copied()
            .unwrap_or(Action::Cooperate)
    }
}

impl Strategy for OneSidedTmStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.tape.clear();
        self.tape.push(self.blank);
        self.head = 0;
        self.last_history_len = 0;
        self.stats = TmRunStats::default();
    }

    fn next_action(&mut self, history: &History, player_a: bool) -> Action {
        self.append_history(history, player_a);

        self.head = self.tape.len().saturating_sub(1);
        let mut state = self.start_state;
        let mut steps_taken: u32 = 0;
        let mut output_symbol: Option<u8> = None;
        let mut fallback = false;
        let mut max_steps_hit = false;

        if self.max_steps_per_round == 0 || state == 0 {
            fallback = true;
        } else {
            let symbols = self.symbols as usize;
            let max_steps = self.max_steps_per_round as usize;
            for _ in 0..max_steps {
                steps_taken += 1;
                let symbol = self.tape.get(self.head).copied().unwrap_or(self.blank);
                let idx = (state.saturating_sub(1) as usize)
                    .saturating_mul(symbols)
                    .saturating_add(symbol as usize);
                let Some(trans) = self.transitions.get(idx).copied() else {
                    fallback = true;
                    break;
                };
                if let Some(cell) = self.tape.get_mut(self.head) {
                    *cell = trans.write;
                }
                if trans.next == 0 {
                    fallback = true;
                    break;
                }
                match trans.move_dir {
                    TmMove::Left => {
                        if self.head > 0 {
                            self.head -= 1;
                        }
                    }
                    TmMove::Stay => {}
                    TmMove::Right => {
                        if self.head + 1 == self.tape.len() {
                            output_symbol = Some(trans.write);
                            break;
                        } else {
                            self.head += 1;
                        }
                    }
                }
                state = trans.next;
            }
            if output_symbol.is_none() && !fallback && steps_taken >= self.max_steps_per_round {
                max_steps_hit = true;
                fallback = true;
            }
        }

        let action = if let Some(symbol) = output_symbol {
            self.output_from_symbol(symbol)
        } else {
            self.fallback_action()
        };

        self.stats.rounds = self.stats.rounds.saturating_add(1);
        self.stats.steps = self.stats.steps.saturating_add(steps_taken as u64);
        if output_symbol.is_some() {
            self.stats.output_events = self.stats.output_events.saturating_add(1);
        }
        if fallback {
            self.stats.fallback = self.stats.fallback.saturating_add(1);
        }
        if max_steps_hit {
            self.stats.max_steps_hits = self.stats.max_steps_hits.saturating_add(1);
        }

        action
    }

    fn tm_stats(&self) -> Option<&TmRunStats> {
        Some(&self.stats)
    }
}
