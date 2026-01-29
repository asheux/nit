use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabId {
    Gol,
    Games,
}

#[derive(Copy, Clone, Debug)]
pub struct LabSpec {
    pub id: LabId,
    pub label: &'static str,
    pub namespace: &'static str,
    pub default_config: &'static str,
}

impl LabId {
    pub fn label(self) -> &'static str {
        match self {
            LabId::Gol => "GOL",
            LabId::Games => "GAMES",
        }
    }

    pub fn namespace(self) -> &'static str {
        match self {
            LabId::Gol => "gol",
            LabId::Games => "games",
        }
    }

    pub fn spec(self) -> LabSpec {
        match self {
            LabId::Gol => LabSpec {
                id: self,
                label: "GOL",
                namespace: "gol",
                default_config: "rules.toml",
            },
            LabId::Games => LabSpec {
                id: self,
                label: "GAMES",
                namespace: "games",
                default_config: "games.toml",
            },
        }
    }
}

impl std::fmt::Display for LabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                LabId::Gol => "gol",
                LabId::Games => "games",
            }
        )
    }
}

pub type AppKind = LabId;
