//! Round-by-round match history and bit-packed rolling memory windows.

use crate::game::{Action, Outcome};

// 2 bits per outcome means a u64 holds at most 31 rounds.
pub const MAX_ROLLING_DEPTH: usize = 31;

#[derive(Copy, Clone, Debug)]
pub struct RoundRecord {
    pub a: Action,
    pub b: Action,
}

impl RoundRecord {
    pub fn oriented_actions(self, player_a: bool) -> (Action, Action) {
        if player_a {
            (self.a, self.b)
        } else {
            (self.b, self.a)
        }
    }
}

impl std::fmt::Display for RoundRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.a.as_char(), self.b.as_char())
    }
}

#[derive(Clone, Debug)]
pub struct History {
    rounds: Vec<RoundRecord>,
    memory_window: RollingHistory,
}

impl History {
    /// `max_memory` is the rolling-window depth; it is silently clamped to [`MAX_ROLLING_DEPTH`].
    pub fn new(max_memory: usize) -> Self {
        Self {
            rounds: Vec::new(),
            memory_window: RollingHistory::new(max_memory),
        }
    }

    pub fn len(&self) -> usize {
        self.rounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rounds.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = RoundRecord> + '_ {
        self.rounds.iter().copied()
    }

    pub fn last(&self) -> Option<RoundRecord> {
        self.rounds.last().copied()
    }

    /// Bit-packed memory index over the last `n` rounds, or `None` when the
    /// window has not yet seen `n` rounds.
    pub fn memory_index(&self, player_a: bool, n: usize) -> Option<usize> {
        self.memory_window.index_for(player_a, n)
    }

    pub fn push(&mut self, a: Action, b: Action) {
        self.rounds.push(RoundRecord { a, b });
        self.memory_window.update(a, b);
    }
}

#[derive(Clone, Debug)]
struct RollingHistory {
    depth: usize,
    // Mask = (1 << (2 * depth)) - 1; truncates each window to the configured depth.
    window_mask: u64,
    window_a: u64,
    window_b: u64,
    rounds_recorded: usize,
}

impl RollingHistory {
    fn new(depth: usize) -> Self {
        let depth = depth.min(MAX_ROLLING_DEPTH);
        let window_mask = if depth == 0 {
            0
        } else {
            (1u64 << (2 * depth)) - 1
        };
        Self {
            depth,
            window_mask,
            window_a: 0,
            window_b: 0,
            rounds_recorded: 0,
        }
    }

    fn update(&mut self, action_a: Action, action_b: Action) {
        if self.depth == 0 {
            return;
        }
        let bits_a = Outcome::from_actions(action_a, action_b).index() as u64;
        let bits_b = Outcome::from_actions(action_b, action_a).index() as u64;
        self.window_a = ((self.window_a << 2) | bits_a) & self.window_mask;
        self.window_b = ((self.window_b << 2) | bits_b) & self.window_mask;
        self.rounds_recorded = self.rounds_recorded.saturating_add(1).min(self.depth);
    }

    fn index_for(&self, player_a: bool, depth: usize) -> Option<usize> {
        if depth == 0 || depth > self.depth || self.rounds_recorded < depth {
            return None;
        }
        let mask = (1u64 << (2 * depth)) - 1;
        let window = if player_a {
            self.window_a
        } else {
            self.window_b
        };
        Some((window & mask) as usize)
    }
}
