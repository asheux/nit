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

    /// Parses case-insensitive action strings: `"c"`, `"coop"`, `"cooperate"`,
    /// `"cooperation"`, `"d"`, `"defect"`, `"defection"`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "c" | "coop" | "cooperate" | "cooperation" => Some(Self::Cooperate),
            "d" | "defect" | "defection" => Some(Self::Defect),
            _ => None,
        }
    }

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

pub const ACTION_COUNT: usize = 2;
pub const OUTCOME_COUNT: usize = 4;

/// Joint outcome of a round: (self_action, opponent_action) encoded as
/// Cooperate/Defect initials.
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

    /// Numeric index in `0..4`, used for lookup-table indexing in FSM strategies.
    pub fn index(self) -> usize {
        match self {
            Self::CC => 0,
            Self::CD => 1,
            Self::DC => 2,
            Self::DD => 3,
        }
    }

    pub fn actions(self) -> (Action, Action) {
        match self {
            Self::CC => (Action::Cooperate, Action::Cooperate),
            Self::CD => (Action::Cooperate, Action::Defect),
            Self::DC => (Action::Defect, Action::Cooperate),
            Self::DD => (Action::Defect, Action::Defect),
        }
    }

    pub fn mirror(self) -> Self {
        match self {
            Self::CC => Self::CC,
            Self::CD => Self::DC,
            Self::DC => Self::CD,
            Self::DD => Self::DD,
        }
    }

    /// ASCII digit byte encoding: CC=`b'0'`, CD=`b'1'`, DC=`b'2'`, DD=`b'3'`.
    pub fn digit_byte(self) -> u8 {
        b'0' + self.index() as u8
    }

    /// ASCII digit char encoding: CC=`'0'`, CD=`'1'`, DC=`'2'`, DD=`'3'`.
    pub fn digit_char(self) -> char {
        char::from(self.digit_byte())
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

/// A 2x2 payoff matrix for a symmetric two-player game.
/// Named fields use standard PD parameterization: R (reward), S (sucker),
/// T (temptation), P (punishment). The `matrix` stores the full
/// `[a_action][b_action] -> [a_payoff, b_payoff]` lookup.
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
        Self::from_matrix([[[-1, -1], [-3, 0]], [[0, -3], [-2, -2]]])
    }

    /// Layout: `[a_choice][b_choice] = [a_payoff, b_payoff]`.
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

    /// Returns `(min, max)` payoff across all cells. Widens the range when
    /// all values are identical to prevent division-by-zero in normalization.
    pub fn min_max(self) -> (i32, i32) {
        let all_values = self.matrix.iter().flatten().flatten().copied();
        let mut lo = all_values.clone().min().unwrap_or(0);
        let hi = all_values.max().unwrap_or(0);
        if lo == hi {
            lo = if lo > 0 { 0 } else { lo.saturating_sub(1) };
        }
        (lo, hi)
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

    let (penalty, bonus) = payoff_matrix.min_max();
    match (player_a_halted, player_b_halted) {
        // Both-halted already handled above by the early return.
        (false, true) => (penalty, bonus),
        (true, false) => (bonus, penalty),
        _ => (penalty, penalty),
    }
}
