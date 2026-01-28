use crate::game::{Action, Outcome};
use crate::history::History;
use nit_utils::hashing::XorShift64;

pub trait Strategy: Send {
    fn id(&self) -> &str;
    fn reset(&mut self);
    fn next_action(&mut self, history: &History, player_a: bool) -> Action;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StrategyKind {
    Builtin,
    Random,
    Fsm,
    Memory,
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
    transitions: Vec<[usize; 4]>,
}

impl FsmStrategy {
    pub fn new(
        id: impl Into<String>,
        start_state: usize,
        outputs: Vec<Action>,
        transitions: Vec<[usize; 4]>,
    ) -> Self {
        Self {
            id: id.into(),
            start_state,
            state: start_state,
            outputs,
            transitions,
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
            let idx = Outcome::from_actions(self_action, opp_action).index();
            if let Some(next) = self.transitions.get(self.state).and_then(|t| t.get(idx)) {
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
