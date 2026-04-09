//! Round-by-round match history and bit-packed rolling memory windows.

use crate::game::{Action, Outcome};

/// 2 bits per outcome, so 31 rounds fit in a `u64`.
pub const MAX_ROLLING_DEPTH: usize = 31;

#[derive(Copy, Clone, Debug)]
pub struct RoundRecord {
    pub a: Action,
    pub b: Action,
}

impl RoundRecord {
    /// Returns `(own_action, opponent_action)` oriented to the given player.
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
    /// Creates a new empty history. `max_memory` controls how many recent
    /// rounds are tracked by the rolling window (capped at 31).
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

    /// Returns the rolling memory index for the given player over the last `n`
    /// rounds, or `None` if insufficient history is available.
    pub fn memory_index(&self, player_a: bool, n: usize) -> Option<usize> {
        self.memory_window.index_for(player_a, n)
    }

    /// Records a new round of play and updates the rolling memory window.
    pub fn push(&mut self, a: Action, b: Action) {
        self.rounds.push(RoundRecord { a, b });
        self.memory_window.update(a, b);
    }
}

/// Bit-packed sliding window: 2 bits per outcome, one window per player perspective.
#[derive(Clone, Debug)]
struct RollingHistory {
    window_depth: usize,
    /// `(1 << (2 * window_depth)) - 1` — truncates the window to the configured depth.
    window_mask: u64,
    window_a: u64,
    window_b: u64,
    rounds_recorded: usize,
}

impl RollingHistory {
    /// Creates an empty rolling window that will track the most recent
    /// `depth` rounds (silently clamped to 31).
    fn new(depth: usize) -> Self {
        let window_depth = depth.min(MAX_ROLLING_DEPTH);

        let window_mask = if window_depth == 0 {
            0
        } else {
            (1u64 << (2 * window_depth)) - 1
        };

        Self {
            window_depth,
            window_mask,
            window_a: 0,
            window_b: 0,
            rounds_recorded: 0,
        }
    }

    /// Shifts a new round's outcomes into both player windows.
    fn update(&mut self, action_a: Action, action_b: Action) {
        if self.window_depth == 0 {
            return;
        }

        let outcome_bits_a = Outcome::from_actions(action_a, action_b).index() as u64;
        let outcome_bits_b = Outcome::from_actions(action_b, action_a).index() as u64;

        self.window_a = ((self.window_a << 2) | outcome_bits_a) & self.window_mask;
        self.window_b = ((self.window_b << 2) | outcome_bits_b) & self.window_mask;

        self.rounds_recorded = self
            .rounds_recorded
            .saturating_add(1)
            .min(self.window_depth);
    }

    /// Returns the memory index for the given player over the last `depth`
    /// rounds, or `None` when fewer than `depth` rounds have been recorded.
    fn index_for(&self, player_a: bool, depth: usize) -> Option<usize> {
        if depth == 0 || depth > self.window_depth || self.rounds_recorded < depth {
            return None;
        }

        let depth_mask = (1u64 << (2 * depth)) - 1;

        let perspective_window = if player_a {
            self.window_a
        } else {
            self.window_b
        };

        Some((perspective_window & depth_mask) as usize)
    }
}
