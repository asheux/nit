//! User-facing configuration: persisted to `~/.config/nit/config.toml` and
//! optionally layered on by a per-workspace `.nit/config.toml` override.
//!
//! Submodules are split by concern. Adding a field anywhere here is a public
//! TOML contract — preserve serde field names and keep `Default` impls
//! returning the same values, otherwise existing user configs silently lose
//! settings on upgrade.

pub mod editor;
pub mod genome;
pub mod gol;
pub mod highlight;
pub mod power;
pub mod settings;
pub mod swarm;

pub use editor::EditorConfig;
pub use genome::{AgentGenomeConfig, GenomeGateConfig};
pub use gol::{
    GolConfig, GolRuleConfig, GolRulesConfig, GolSearchConfig, GolSearchIntensity, GolSeedSource,
    GolSnapshotsConfig, GolUserRule, SnapshotPrunePolicy,
};
pub use highlight::{HighlightConfig, HighlightEngine};
pub use power::PowerConfig;
pub use settings::Settings;
pub use swarm::SwarmConfig;

/// Shared `#[serde(default = "...")]` helper for boolean fields whose
/// historical default is `true`. Centralized so adding a new opt-out flag
/// does not require yet another sibling-module helper.
pub(crate) fn default_true() -> bool {
    true
}

#[cfg(test)]
#[path = "../tests/config.rs"]
mod tests;
