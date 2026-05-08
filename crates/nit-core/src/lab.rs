//! Lab namespace registry — `LabId` discriminates between the GoL and
//! Games subsystems. The `label`/`namespace`/`default_config` triples
//! land in user-facing UI and on-disk paths, so the strings here are the
//! contract: `namespace` is also the directory name under
//! `<workspace>/.nit/<namespace>/`.

use std::fmt;

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

impl fmt::Display for LabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.namespace())
    }
}

/// Backwards-compatible alias: the `AppState::app_kind` field is named
/// after this type's earlier role as the app-wide discriminator.
pub type AppKind = LabId;
