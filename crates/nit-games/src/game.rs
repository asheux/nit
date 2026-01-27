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
}

impl PayoffMatrix {
    pub fn default_pd() -> Self {
        Self {
            r: 3,
            s: 0,
            t: 5,
            p: 1,
        }
    }

    pub fn payoffs(self, a: Action, b: Action) -> (i32, i32) {
        match (a, b) {
            (Action::Cooperate, Action::Cooperate) => (self.r, self.r),
            (Action::Cooperate, Action::Defect) => (self.s, self.t),
            (Action::Defect, Action::Cooperate) => (self.t, self.s),
            (Action::Defect, Action::Defect) => (self.p, self.p),
        }
    }
}
