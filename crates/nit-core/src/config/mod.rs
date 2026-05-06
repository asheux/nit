pub mod editor;
pub mod genome;
pub mod gol;
pub mod highlight;
pub mod settings;
pub mod swarm;

pub use editor::EditorConfig;
pub use genome::{AgentGenomeConfig, GenomeGateConfig};
pub use gol::{
    GolConfig, GolRuleConfig, GolRulesConfig, GolSearchConfig, GolSearchIntensity, GolSeedSource,
    GolSnapshotsConfig, GolUserRule, SnapshotPrunePolicy,
};
pub use highlight::{HighlightConfig, HighlightEngine};
pub use settings::Settings;
pub use swarm::SwarmConfig;

pub(crate) fn default_true() -> bool {
    true
}

#[cfg(test)]
#[path = "../tests/config.rs"]
mod tests;
