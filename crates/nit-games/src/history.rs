use crate::game::{Action, Outcome};

#[derive(Copy, Clone, Debug)]
pub struct RoundRecord {
    pub a: Action,
    pub b: Action,
}

#[derive(Clone, Debug)]
pub struct History {
    rounds: Vec<RoundRecord>,
    rolling: RollingHistory,
}

impl History {
    pub fn new(max_memory: usize) -> Self {
        Self {
            rounds: Vec::new(),
            rolling: RollingHistory::new(max_memory),
        }
    }

    pub fn len(&self) -> usize {
        self.rounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rounds.is_empty()
    }

    pub fn last(&self) -> Option<RoundRecord> {
        self.rounds.last().copied()
    }

    pub fn last_outcome_for(&self, player_a: bool) -> Option<Outcome> {
        let last = self.last()?;
        let (self_action, opp_action) = if player_a {
            (last.a, last.b)
        } else {
            (last.b, last.a)
        };
        Some(Outcome::from_actions(self_action, opp_action))
    }

    pub fn last_actions_for(&self, player_a: bool) -> Option<(Action, Action)> {
        let last = self.last()?;
        let (self_action, opp_action) = if player_a {
            (last.a, last.b)
        } else {
            (last.b, last.a)
        };
        Some((self_action, opp_action))
    }

    pub fn last_opponent_action(&self, player_a: bool) -> Option<Action> {
        let last = self.last()?;
        Some(if player_a { last.b } else { last.a })
    }

    pub fn memory_index(&self, player_a: bool, n: usize) -> Option<usize> {
        self.rolling.index_for(player_a, n)
    }

    pub fn push(&mut self, a: Action, b: Action) {
        self.rounds.push(RoundRecord { a, b });
        self.rolling.update(a, b);
    }
}

#[derive(Clone, Debug)]
struct RollingHistory {
    max_n: usize,
    mask: u64,
    window_a: u64,
    window_b: u64,
    filled: usize,
}

impl RollingHistory {
    fn new(max_n: usize) -> Self {
        let max_n = max_n.min(31);
        let mask = if max_n == 0 {
            0
        } else {
            (1u64 << (2 * max_n)) - 1
        };
        Self {
            max_n,
            mask,
            window_a: 0,
            window_b: 0,
            filled: 0,
        }
    }

    fn update(&mut self, a: Action, b: Action) {
        if self.max_n == 0 {
            return;
        }
        let outcome_a = Outcome::from_actions(a, b).index() as u64;
        let outcome_b = Outcome::from_actions(b, a).index() as u64;
        self.window_a = ((self.window_a << 2) | outcome_a) & self.mask;
        self.window_b = ((self.window_b << 2) | outcome_b) & self.mask;
        self.filled = (self.filled + 1).min(self.max_n);
    }

    fn index_for(&self, player_a: bool, n: usize) -> Option<usize> {
        if n == 0 || n > self.max_n || self.filled < n {
            return None;
        }
        let mask = (1u64 << (2 * n)) - 1;
        let window = if player_a {
            self.window_a
        } else {
            self.window_b
        };
        Some((window & mask) as usize)
    }
}
