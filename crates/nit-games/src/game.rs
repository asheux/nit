//! Core game primitives for iterated two-player games.

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Cooperate,
    Defect,
}

impl Action {
    pub fn as_char(self) -> char {
        match self {
            Self::Cooperate => 'C',
            Self::Defect => 'D',
        }
    }

    /// Parses case-insensitive `c`/`coop`/`cooperate`/`cooperation` and
    /// `d`/`defect`/`defection`. Whitespace is trimmed.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "c" | "coop" | "cooperate" | "cooperation" => Some(Self::Cooperate),
            "d" | "defect" | "defection" => Some(Self::Defect),
            _ => None,
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

pub const ACTION_COUNT: usize = 2;
pub const OUTCOME_COUNT: usize = 4;

/// Joint outcome of a round: `(self_action, opponent_action)` encoded as
/// Cooperate/Defect initials. The numeric `index()` (0..4) doubles as an
/// ASCII-digit offset, used by FSM lookup tables and history-string scores.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    CC,
    CD,
    DC,
    DD,
}

impl Outcome {
    pub const ALL: [Self; OUTCOME_COUNT] = [Self::CC, Self::CD, Self::DC, Self::DD];

    pub fn from_actions(self_action: Action, opponent_action: Action) -> Self {
        match (self_action, opponent_action) {
            (Action::Cooperate, Action::Cooperate) => Self::CC,
            (Action::Cooperate, Action::Defect) => Self::CD,
            (Action::Defect, Action::Cooperate) => Self::DC,
            (Action::Defect, Action::Defect) => Self::DD,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::CC => 0,
            Self::CD => 1,
            Self::DC => 2,
            Self::DD => 3,
        }
    }

    /// ASCII digit byte: CC=`b'0'`, CD=`b'1'`, DC=`b'2'`, DD=`b'3'`. Used to
    /// build the compact per-round outcome string in the history log.
    pub fn digit_byte(self) -> u8 {
        b'0' + self.index() as u8
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CC => "CC",
            Self::CD => "CD",
            Self::DC => "DC",
            Self::DD => "DD",
        })
    }
}

/// 2x2 payoff matrix for a symmetric two-player game. Named fields use the
/// standard PD parameterization (R, S, T, P); `matrix` stores the full
/// `[a_action][b_action] -> [a_payoff, b_payoff]` lookup, which is the
/// shape consumed by the GPU shaders and the fast-eval path.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct PayoffMatrix {
    pub r: i32,
    pub s: i32,
    pub t: i32,
    pub p: i32,
    pub matrix: [[[i32; 2]; 2]; 2],
}

impl PayoffMatrix {
    pub fn default_pd() -> Self {
        Self {
            r: -1,
            s: -3,
            t: 0,
            p: -2,
            matrix: [[[-1, -1], [-3, 0]], [[0, -3], [-2, -2]]],
        }
    }

    /// Layout: `[a_choice][b_choice] = [a_payoff, b_payoff]`. Reads R/S/T/P
    /// out of the canonical `(0,0) / (0,1) / (1,0) / (1,1)` cells.
    pub fn from_matrix(raw: [[[i32; 2]; 2]; 2]) -> Self {
        Self {
            r: raw[0][0][0],
            s: raw[0][1][0],
            t: raw[1][0][0],
            p: raw[1][1][0],
            matrix: raw,
        }
    }

    pub fn payoffs(self, player_a: Action, player_b: Action) -> (i32, i32) {
        let cell = self.matrix[player_a as usize][player_b as usize];
        (cell[0], cell[1])
    }

    /// Returns `(min, max)` payoff across all cells. When every cell is
    /// identical the lower bound is widened by one (or anchored at zero
    /// when all values are positive) so downstream normalization can
    /// divide by the span without hitting zero.
    pub fn min_max(self) -> (i32, i32) {
        let (lo, hi) = self
            .matrix
            .iter()
            .flatten()
            .flatten()
            .copied()
            .fold((i32::MAX, i32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        if lo != hi {
            return (lo, hi);
        }
        let widened_lo = if lo > 0 { 0 } else { lo.saturating_sub(1) };
        (widened_lo, hi)
    }
}

/// Computes payoffs for a round, applying timeout penalties when a strategy
/// has not halted (e.g. a non-halting Turing machine). The non-halting side
/// receives the matrix minimum; the halting side receives the maximum.
pub fn payoffs_with_timeouts(
    payoff_matrix: PayoffMatrix,
    player_a_action: Action,
    player_b_action: Action,
    player_a_halted: bool,
    player_b_halted: bool,
) -> (i32, i32) {
    if player_a_halted && player_b_halted {
        return payoff_matrix.payoffs(player_a_action, player_b_action);
    }

    let (min_payoff, max_payoff) = payoff_matrix.min_max();
    match (player_a_halted, player_b_halted) {
        (true, false) => (max_payoff, min_payoff),
        (false, true) => (min_payoff, max_payoff),
        _ => (min_payoff, min_payoff),
    }
}
