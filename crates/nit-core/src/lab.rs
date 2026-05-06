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

    pub fn label(self) -> &'static str {
        self.spec().label
    }

    pub fn namespace(self) -> &'static str {
        self.spec().namespace
    }
}

impl std::fmt::Display for LabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.namespace())
    }
}

pub type AppKind = LabId;
