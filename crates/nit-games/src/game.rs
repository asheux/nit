use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Cooperate,
    Defect,
}

impl Action {
    pub fn as_char(self) -> char {
        match self {
            Action::Cooperate => 'C',
            Action::Defect => 'D',
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "c" | "coop" | "cooperate" | "cooperation" => Some(Action::Cooperate),
            "d" | "defect" | "defection" => Some(Action::Defect),
            _ => None,
        }
    }

    pub fn flip(self) -> Self {
        match self {
            Action::Cooperate => Action::Defect,
            Action::Defect => Action::Cooperate,
        }
    }
}

impl std::str::FromStr for Action {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Action::parse(s).ok_or(())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    CC,
    CD,
    DC,
    DD,
}

impl Outcome {
    pub fn from_actions(self_action: Action, opp_action: Action) -> Self {
        match (self_action, opp_action) {
            (Action::Cooperate, Action::Cooperate) => Outcome::CC,
            (Action::Cooperate, Action::Defect) => Outcome::CD,
            (Action::Defect, Action::Cooperate) => Outcome::DC,
            (Action::Defect, Action::Defect) => Outcome::DD,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Outcome::CC => 0,
            Outcome::CD => 1,
            Outcome::DC => 2,
            Outcome::DD => 3,
        }
    }
}

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
        let matrix = [[[-1, -1], [-3, 0]], [[0, -3], [-2, -2]]];
        Self::from_matrix(matrix)
    }

    pub fn from_matrix(matrix: [[[i32; 2]; 2]; 2]) -> Self {
        let r = matrix[0][0][0];
        let s = matrix[0][1][0];
        let t = matrix[1][0][0];
        let p = matrix[1][1][0];
        Self { r, s, t, p, matrix }
    }

    pub fn payoffs(self, a: Action, b: Action) -> (i32, i32) {
        let a_idx = match a {
            Action::Cooperate => 0,
            Action::Defect => 1,
        };
        let b_idx = match b {
            Action::Cooperate => 0,
            Action::Defect => 1,
        };
        let cell = self.matrix[a_idx][b_idx];
        (cell[0], cell[1])
    }

    pub fn min_max(self) -> (i32, i32) {
        let mut min_value = i32::MAX;
        let mut max_value = i32::MIN;
        for row in self.matrix {
            for cell in row {
                for value in cell {
                    min_value = min_value.min(value);
                    max_value = max_value.max(value);
                }
            }
        }
        if min_value == max_value {
            if min_value > 0 {
                min_value = 0;
            } else {
                min_value = min_value.saturating_sub(1);
            }
        }
        (min_value, max_value)
    }
}

pub fn payoffs_with_timeouts(
    payoff: PayoffMatrix,
    a_action: Action,
    b_action: Action,
    a_halted: bool,
    b_halted: bool,
) -> (i32, i32) {
    if a_halted && b_halted {
        return payoff.payoffs(a_action, b_action);
    }
    let (lose, win) = payoff.min_max();
    match (a_halted, b_halted) {
        (true, true) => payoff.payoffs(a_action, b_action),
        (false, true) => (lose, win),
        (true, false) => (win, lose),
        (false, false) => (lose, lose),
    }
}
