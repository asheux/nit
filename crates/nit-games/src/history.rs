//! Round-by-round match history and bit-packed rolling memory windows.
//!
//! [`History`] stores the full round-by-round record of a match and
//! maintains a [`RollingHistory`] sliding window for O(1) memory-index
//! lookups used by FSM and memory-based strategies.

use crate::game::{Action, Outcome};

// ── Constants ──────────────────────────────────────────────────────────

/// Maximum number of recent rounds that can be packed into a `u64` rolling
/// window (2 bits per outcome).
pub const MAX_ROLLING_DEPTH: usize = 31;

// ── Round record ───────────────────────────────────────────────────────

/// A single round's actions for both players.
#[derive(Copy, Clone, Debug)]
pub struct RoundRecord {
    /// Player A's action in this round.
    pub a: Action,
    /// Player B's action in this round.
    pub b: Action,
}

impl RoundRecord {
    /// Returns the outcome from player A's perspective.
    pub fn outcome_for_a(self) -> Outcome {
        Outcome::from_actions(self.a, self.b)
    }

    /// Returns the outcome from player B's perspective.
    pub fn outcome_for_b(self) -> Outcome {
        Outcome::from_actions(self.b, self.a)
    }
}

impl std::fmt::Display for RoundRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.a.as_char(), self.b.as_char())
    }
}

// ── Match history ──────────────────────────────────────────────────────

/// Full match history with round-by-round records and a rolling memory window
/// for efficient strategy lookups.
///
/// The `memory_window` maintains a bit-packed sliding view over recent outcomes,
/// allowing O(1) memory-index queries used by FSM and memory-based strategies.
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

    /// Returns the number of rounds played so far.
    pub fn len(&self) -> usize {
        self.rounds.len()
    }

    /// Returns `true` if no rounds have been played.
    pub fn is_empty(&self) -> bool {
        self.rounds.is_empty()
    }

    /// Iterates over all round records in chronological order.
    pub fn iter(&self) -> impl Iterator<Item = RoundRecord> + '_ {
        self.rounds.iter().copied()
    }

    /// Returns the most recent round record, if any.
    pub fn last(&self) -> Option<RoundRecord> {
        self.rounds.last().copied()
    }

    /// Returns the most recent outcome from the given player's perspective.
    ///
    /// When `player_a` is `true` the outcome is oriented so that player A's
    /// action is "self" and player B's action is "opponent", and vice versa.
    pub fn last_outcome_for(&self, player_a: bool) -> Option<Outcome> {
        let most_recent = self.last()?;

        let (own_action, opponent_action) = if player_a {
            (most_recent.a, most_recent.b)
        } else {
            (most_recent.b, most_recent.a)
        };

        Some(Outcome::from_actions(own_action, opponent_action))
    }

    /// Returns `(self_action, opponent_action)` for the most recent round,
    /// oriented to the given player's perspective.
    pub fn last_actions_for(&self, player_a: bool) -> Option<(Action, Action)> {
        let most_recent = self.last()?;

        if player_a {
            Some((most_recent.a, most_recent.b))
        } else {
            Some((most_recent.b, most_recent.a))
        }
    }

    /// Returns the opponent's most recent action from the given player's perspective.
    pub fn last_opponent_action(&self, player_a: bool) -> Option<Action> {
        let most_recent = self.last()?;
        Some(if player_a {
            most_recent.b
        } else {
            most_recent.a
        })
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

// ── Rolling memory window (internal) ───────────────────────────────────

/// Bit-packed sliding window over recent outcomes for O(1) memory-index lookups.
///
/// Each outcome occupies 2 bits (4 possible `Outcome` variants), so up to 31
/// rounds of history can be packed into a single `u64`.  Two separate windows
/// are maintained -- one per player perspective -- so that strategy evaluation
/// always sees outcomes oriented to the correct side.
#[derive(Clone, Debug)]
struct RollingHistory {
    /// Maximum number of recent rounds tracked (capped at 31 to fit within `u64`).
    window_depth: usize,

    /// Bitmask that truncates the rolling window to `window_depth` outcomes:
    /// `(1 << (2 * window_depth)) - 1`.
    window_mask: u64,

    /// Rolling outcome bits from player A's perspective.
    window_a: u64,

    /// Rolling outcome bits from player B's perspective.
    window_b: u64,

    /// Number of rounds recorded so far (saturates at `window_depth`).
    rounds_recorded: usize,
}

impl RollingHistory {
    /// Creates an empty rolling window that will track the most recent
    /// `depth` rounds (silently clamped to 31).
    fn new(depth: usize) -> Self {
        let window_depth = depth.min(31);

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

        self.rounds_recorded = (self.rounds_recorded + 1).min(self.window_depth);
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
