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

    pub fn from_str(value: &str) -> Option<Self> {
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
        let matrix = [[[3, 3], [0, 5]], [[5, 0], [1, 1]]];
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
}
