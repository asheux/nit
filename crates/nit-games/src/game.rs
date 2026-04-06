//! Core game primitives for iterated two-player games.
//!
//! This module defines the fundamental types — [`Action`], [`Outcome`], and
//! [`PayoffMatrix`] — that underpin every strategy evaluation and tournament
//! match in the crate.

use serde::{Deserialize, Serialize};

// ── Player actions ──────────────────────────────────────────────────────

/// A player's action in a single round of the iterated game.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Cooperate,
    Defect,
}

impl Action {
    /// Returns the single-character representation (`'C'` or `'D'`).
    pub fn as_char(self) -> char {
        match self {
            Self::Cooperate => 'C',
            Self::Defect => 'D',
        }
    }

    /// Parses a human-friendly string into an [`Action`].
    ///
    /// Accepts case-insensitive variants: `"c"`, `"coop"`, `"cooperate"`,
    /// `"cooperation"`, `"d"`, `"defect"`, `"defection"`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "c" | "coop" | "cooperate" | "cooperation" => Some(Self::Cooperate),
            "d" | "defect" | "defection" => Some(Self::Defect),
            _ => None,
        }
    }

    /// Returns the opposite action.
    pub fn flip(self) -> Self {
        match self {
            Self::Cooperate => Self::Defect,
            Self::Defect => Self::Cooperate,
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cooperate => "Cooperate",
            Self::Defect => "Defect",
        })
    }
}

impl std::str::FromStr for Action {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or(())
    }
}

// ── Action-level constants ──────────────────────────────────────────────

/// Total number of distinct actions available to a player.
pub const ACTION_COUNT: usize = 2;

/// Total number of distinct joint outcomes in a two-player round.
pub const OUTCOME_COUNT: usize = 4;

// ── Joint outcomes ─────────────────────────────────────────────────────

/// The combined outcome of a round, from the perspective of both players.
///
/// Each variant encodes (self_action, opponent_action) as a pair of
/// Cooperate/Defect initials: `CC`, `CD`, `DC`, `DD`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    CC,
    CD,
    DC,
    DD,
}

impl Outcome {
    /// All four outcomes in index order, for iteration and table construction.
    pub const ALL: [Self; OUTCOME_COUNT] = [Self::CC, Self::CD, Self::DC, Self::DD];

    /// Constructs an [`Outcome`] from the two players' actions.
    pub fn from_actions(self_action: Action, opp_action: Action) -> Self {
        match (self_action, opp_action) {
            (Action::Cooperate, Action::Cooperate) => Self::CC,
            (Action::Cooperate, Action::Defect) => Self::CD,
            (Action::Defect, Action::Cooperate) => Self::DC,
            (Action::Defect, Action::Defect) => Self::DD,
        }
    }

    /// Returns a numeric index in `0..4` for this outcome, useful for
    /// lookup-table indexing in FSM / memory-based strategies.
    pub fn index(self) -> usize {
        match self {
            Self::CC => 0,
            Self::CD => 1,
            Self::DC => 2,
            Self::DD => 3,
        }
    }

    /// Decomposes this outcome into `(self_action, opponent_action)`.
    pub fn actions(self) -> (Action, Action) {
        match self {
            Self::CC => (Action::Cooperate, Action::Cooperate),
            Self::CD => (Action::Cooperate, Action::Defect),
            Self::DC => (Action::Defect, Action::Cooperate),
            Self::DD => (Action::Defect, Action::Defect),
        }
    }

    /// Returns the mirrored outcome as seen from the opponent's perspective.
    pub fn mirror(self) -> Self {
        match self {
            Self::CC => Self::CC,
            Self::CD => Self::DC,
            Self::DC => Self::CD,
            Self::DD => Self::DD,
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = match self {
            Self::CC => "CC",
            Self::CD => "CD",
            Self::DC => "DC",
            Self::DD => "DD",
        };
        f.write_str(tag)
    }
}

// ── Payoff matrix ──────────────────────────────────────────────────────

/// A 2x2 payoff matrix for a symmetric two-player game.
///
/// The four named fields correspond to the classical Prisoner's Dilemma
/// parameterization:
/// - `R` (reward for mutual cooperation)
/// - `S` (sucker's payoff)
/// - `T` (temptation to defect)
/// - `P` (punishment for mutual defection)
///
/// The full `matrix` stores `[player_a_action][player_b_action] -> [a_payoff, b_payoff]`.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct PayoffMatrix {
    /// Reward: payoff when both players cooperate (CC).
    pub r: i32,
    /// Sucker's payoff: payoff to the cooperator when the opponent defects (CD).
    pub s: i32,
    /// Temptation: payoff to the defector when the opponent cooperates (DC).
    pub t: i32,
    /// Punishment: payoff when both players defect (DD).
    pub p: i32,
    /// Full payoff lookup: `matrix[a_action][b_action] = [a_payoff, b_payoff]`.
    pub matrix: [[[i32; 2]; 2]; 2],
}

impl PayoffMatrix {
    /// Returns the standard Prisoner's Dilemma matrix: `R=-1, S=-3, T=0, P=-2`.
    pub fn default_pd() -> Self {
        let raw_matrix = [[[-1, -1], [-3, 0]], [[0, -3], [-2, -2]]];
        Self::from_matrix(raw_matrix)
    }

    /// Constructs a [`PayoffMatrix`] from a raw 2x2x2 array, extracting the
    /// named payoff parameters (`r`, `s`, `t`, `p`) from the appropriate cells.
    ///
    /// The array layout is `[player_a_choice][player_b_choice] = [a_payoff, b_payoff]`.
    pub fn from_matrix(raw_matrix: [[[i32; 2]; 2]; 2]) -> Self {
        let reward = raw_matrix[0][0][0];
        let sucker = raw_matrix[0][1][0];
        let temptation = raw_matrix[1][0][0];
        let punishment = raw_matrix[1][1][0];

        Self {
            r: reward,
            s: sucker,
            t: temptation,
            p: punishment,
            matrix: raw_matrix,
        }
    }

    /// Returns the `(a_payoff, b_payoff)` for the given pair of actions.
    pub fn payoffs(self, player_a_action: Action, player_b_action: Action) -> (i32, i32) {
        let row = player_a_action as usize;
        let col = player_b_action as usize;
        let cell = self.matrix[row][col];

        (cell[0], cell[1])
    }

    /// Returns the `(min, max)` payoff values across all cells of the matrix.
    ///
    /// When all values are identical the range is widened so that `min < max`,
    /// which prevents division-by-zero in downstream normalization.
    pub fn min_max(self) -> (i32, i32) {
        let all_payoff_values = self.matrix.iter().flatten().flatten().copied();

        let mut lower_bound = all_payoff_values.clone().min().unwrap_or(0);
        let upper_bound = all_payoff_values.max().unwrap_or(0);

        // Widen the range when all payoffs are identical to avoid zero-width intervals.
        if lower_bound == upper_bound {
            lower_bound = if lower_bound > 0 {
                0
            } else {
                lower_bound.saturating_sub(1)
            };
        }

        (lower_bound, upper_bound)
    }
}

// ── Timeout-aware payoff resolution ─────────────────────────────────────

/// Computes payoffs for a round, applying timeout penalties when a strategy
/// has not halted (e.g. a non-halting Turing machine).
///
/// If both strategies halted normally the standard payoff matrix applies.
/// Otherwise the non-halting side receives the matrix minimum and the halting
/// side receives the maximum.
pub fn payoffs_with_timeouts(
    payoff_matrix: PayoffMatrix,
    player_a_action: Action,
    player_b_action: Action,
    player_a_halted: bool,
    player_b_halted: bool,
) -> (i32, i32) {
    // Both strategies terminated normally -- use the standard payoff table.
    if player_a_halted && player_b_halted {
        return payoff_matrix.payoffs(player_a_action, player_b_action);
    }

    // At least one strategy timed out -- apply penalty scoring.
    let (penalty_score, reward_score) = payoff_matrix.min_max();

    match (player_a_halted, player_b_halted) {
        (true, true) => payoff_matrix.payoffs(player_a_action, player_b_action),
        (false, true) => (penalty_score, reward_score),
        (true, false) => (reward_score, penalty_score),
        (false, false) => (penalty_score, penalty_score),
    }
}
